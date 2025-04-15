use crate::withdraw_from_treasury_to_player;
use crate::GameError;
use crate::RevealAndClaim;
use crate::FINAL_CLAIM_DEADLINE_TIMESTAMP;
use crate::PAYOUT_MULTIPLIER_LUT;
use crate::PAYOUT_SCALE;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::keccak;

pub fn reveal_and_claim(ctx: Context<RevealAndClaim>, bet_value: u8, salt: u64) -> Result<()> {
    require!(bet_value <= 100, GameError::InvalidBetValue);
    let game = &mut ctx.accounts.game;
    let commitment_account = &mut ctx.accounts.bet_commitment;
    let player = *ctx.accounts.player.key;
    let Some(true_result) = game.result else {
        return Err(GameError::ResultNotSubmitted.into());
    };
    let bet_amount = commitment_account.amount;
    // validate the bet value and salt, revealing the bet value
    let mut hasher = keccak::Hasher::default();
    hasher.hash(&bet_value.to_le_bytes());
    hasher.hash(&salt.to_le_bytes());
    let hashed = hasher.result().to_bytes();
    require!(
        hashed == commitment_account.commitment,
        GameError::CommitmentMismatch
    );
    msg!(
        "Bet reveal verified for player: {} (Bet: {}, Salt: {}, Amount: {})",
        player,
        bet_value,
        salt,
        bet_amount
    );

    // --- Claim Logic --- //

    // LOSS CASE - OVER BET THE TRUE RESULT
    if bet_value > true_result {
        // payout is zero, this is a loss since user bet OVER the true result. Host keeps the bet amount.
        msg!("Player lost, no payout {}. Bet marked as settled.", player);
        // player is exiting the pot, decrementing the initial staked bet from total player pot
        game.total_player_pot = game
            .total_player_pot
            .checked_sub(bet_amount)
            .ok_or(GameError::TotalPayoutPotDesynced)?;
        return Ok(());
    }

    // WIN CASE - AT LEAST EATEN X NUGGETS
    // since we claim 0 <= guessed_bet <= 100 previously, sanity check max difference is 100
    let difference = (true_result - bet_value) as usize;
    require!(
        difference < PAYOUT_MULTIPLIER_LUT.len(),
        GameError::InvalidBetValue
    );
    let scaled_multiplier = PAYOUT_MULTIPLIER_LUT[difference];
    let payout_amount =
        ((bet_amount as u128 * scaled_multiplier as u128) / (PAYOUT_SCALE as u128)) as u64;
    msg!(
        "Player {} qualifies for payout. Diff: {}, Multiplier (scaled): {}, Bet: {}, Payout: {}",
        player,
        difference,
        scaled_multiplier,
        bet_amount,
        payout_amount
    );
    // this actually never gets ran as exponential payout curve is > 0, keeping here for sanity
    if payout_amount == 0 {
        // if payout is zero, effectively a loss. Host keeps the bet amount.
        msg!("No payout for player {}. Bet marked as settled.", player);
        // player is exiting the pot, decrementing the initial staked bet from total player pot
        game.total_player_pot = game
            .total_player_pot
            .checked_sub(bet_amount)
            .ok_or(GameError::TotalPayoutPotDesynced)?;
        return Ok(());
    }

    // Check host liquidity implicitly
    let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();

    // this should represent the portion of liquidity that is the host's pool. NOT USING OTHER CONTESTANT'S MONEY!!!! so they can always reclaim their initial stake
    // total_player_pot can NEVER exceed treasury_balance as it should be backed one to one. treasury MUST NOT withdraw anywhere else without subtracting total_player_pot
    let host_liquidity = treasury_balance
        .checked_sub(game.total_player_pot)
        .ok_or(GameError::TotalPayoutPotDesynced)?;
    if payout_amount > host_liquidity {
        // host liquidity insufficient, player can use [`withdraw_unpaid_bet`] to reclaim their bet later if host does not fund...
        commitment_account.attempted_reveal = true;
        commitment_account.is_claimed = false;
        // set the final claim deadline so player can reclaim their initial stake later if host does not fund
        // we don't handle potentially splittng treasury amongst players as thats a bit complicated. lets assume im at least that trustworthy
        game.final_claim_deadline = Some(FINAL_CLAIM_DEADLINE_TIMESTAMP);
        msg!("Host liquidity insufficient for payout. Player can use withdraw_unpaid_bet to reclaim their bet.");
        return Ok(());
    }
    // updating total_player_pot to reflect the payout, decrementing initial stake so remaining comes out of host's liquidity
    game.total_player_pot = game
        .total_player_pot
        .checked_sub(bet_amount)
        .ok_or(GameError::TotalPayoutPotDesynced)?;

    commitment_account.is_claimed = true;
    // perform payout
    msg!(
        "Implicit host liquidity sufficient ({} >= {}). Proceeding with transfer.",
        host_liquidity,
        payout_amount
    );
    withdraw_from_treasury_to_player(
        game,
        &ctx.accounts.game_treasury,
        &ctx.accounts.system_program,
        &ctx.accounts.player,
        payout_amount,
    )?;

    msg!("Transferred payout {} to player {}. Bet marked as settled. Player should call CleanupBetCommitment to reclaim rent.", payout_amount, player);
    Ok(())
}
