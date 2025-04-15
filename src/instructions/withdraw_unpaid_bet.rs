use crate::withdraw_from_treasury_to_player;
use crate::GameError;
use crate::WithdrawUnpaidBet;
use anchor_lang::prelude::*;

pub fn withdraw_unpaid_bet(ctx: Context<WithdrawUnpaidBet>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    let commitment = &ctx.accounts.bet_commitment;
    let player = *ctx.accounts.player.key;

    let reclaim_amount = commitment.amount;
    let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
    // Check if player's original bet amount is still in the treasury
    require!(
        treasury_balance >= reclaim_amount,
        GameError::InsufficientTreasuryForReclaim
    );

    // updating total_player_pot to reflect the payout, decrementing initial stake so remaining comes out of host's liquidity
    game.total_player_pot = game
        .total_player_pot
        .checked_sub(reclaim_amount)
        .ok_or(GameError::TotalPayoutPotDesynced)?;

    // Transfer original bet back to player
    withdraw_from_treasury_to_player(
        game,
        &ctx.accounts.game_treasury,
        &ctx.accounts.system_program,
        &ctx.accounts.player,
        reclaim_amount,
    )?;
    msg!(
        "Host lacked liquidity. Withdrew original bet {} lamports for player {}.",
        reclaim_amount,
        player
    );
    Ok(())
}
