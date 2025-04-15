use crate::withdraw_from_treasury_to_player;
use crate::GameError;
use crate::ReclaimBetOnTimeout;
use anchor_lang::prelude::*;

pub fn reclaim_bet_on_timeout(ctx: Context<ReclaimBetOnTimeout>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    let commitment = &ctx.accounts.bet_commitment;
    let player = *ctx.accounts.player.key;

    let reclaim_amount = commitment.amount;
    let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
    // woops, casino bankrupt ggs. contact me for payout? guess this really trusts the authority
    // ensure liquidity in treasury is high enough to cover all bets before making your bets!
    require!(
        treasury_balance >= reclaim_amount,
        GameError::InsufficientTreasuryForReclaim
    );

    // updating total_player_pot to reflect the payout, decrementing initial stake so remaining comes out of host's liquidity
    game.total_player_pot = game
        .total_player_pot
        .checked_sub(reclaim_amount)
        .ok_or(GameError::TotalPayoutPotDesynced)?;

    withdraw_from_treasury_to_player(
        game,
        &ctx.accounts.game_treasury,
        &ctx.accounts.system_program,
        &ctx.accounts.player,
        reclaim_amount,
    )?;

    msg!(
        "Authority missed deadline. Reclaimed {} lamports for player {}.",
        reclaim_amount,
        player
    );
    msg!("Closing commitment account and returning rent to player.");
    Ok(())
}
