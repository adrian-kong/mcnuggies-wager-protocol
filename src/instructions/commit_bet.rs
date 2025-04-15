use crate::CommitBet;
use crate::GameError;
use anchor_lang::prelude::*;
use anchor_lang::solana_program::native_token::LAMPORTS_PER_SOL;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::system_instruction;

pub fn commit_bet(ctx: Context<CommitBet>, commitment: [u8; 32], amount: u64) -> Result<()> {
    // limit bet range to 0 to 1 sol
    require!(
        0 < amount && amount <= LAMPORTS_PER_SOL,
        GameError::InvalidBetAmount
    );
    let game = &mut ctx.accounts.game;
    let bet_commitment = &mut ctx.accounts.bet_commitment;
    // --- Rest of the commit logic ---
    invoke_signed(
        &system_instruction::transfer(
            ctx.accounts.player.key,
            ctx.accounts.game_treasury.key,
            amount,
        ),
        &[
            ctx.accounts.player.to_account_info(),
            ctx.accounts.game_treasury.to_account_info(),
            ctx.accounts.system_program.to_account_info(),
        ],
        &[],
    )?;

    bet_commitment.player = *ctx.accounts.player.key;
    bet_commitment.commitment = commitment;
    bet_commitment.game = *game.to_account_info().key;
    bet_commitment.amount = amount;
    bet_commitment.attempted_reveal = false;

    game.bet_count = game.bet_count.checked_add(1).ok_or(GameError::Overflow)?;
    game.total_player_pot = game
        .total_player_pot
        .checked_add(amount)
        .ok_or(GameError::Overflow)?;

    msg!(
        "Bet committed by player: {} for amount: {}",
        bet_commitment.player,
        amount,
    );
    Ok(())
}
