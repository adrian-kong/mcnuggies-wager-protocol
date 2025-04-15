use crate::withdraw_from_treasury_to_player;
use crate::ClaimRemainingTreasury;
use anchor_lang::prelude::*;

pub fn claim_remaining_treasury(ctx: Context<ClaimRemainingTreasury>) -> Result<()> {
    let game = &mut ctx.accounts.game;
    // provided authority from the signer
    let authority = *ctx.accounts.authority.key;
    let game_treasury = &ctx.accounts.game_treasury;
    let treasury_balance = game_treasury.to_account_info().lamports();
    if treasury_balance != 0 {
        withdraw_from_treasury_to_player(
            game,
            game_treasury,
            &ctx.accounts.system_program,
            &ctx.accounts.authority,
            treasury_balance,
        )?;
        msg!(
            "Reveal deadline passed. Claimed implicit host liquidity {} lamports from treasury for authority {}. Remaining player pot obligation: {}.",
            treasury_balance,
            authority,
            game.total_player_pot // Log remaining player funds obligation
        );
    } else {
        msg!("Treasury is empty, nothing to claim.");
    }
    Ok(())
}
