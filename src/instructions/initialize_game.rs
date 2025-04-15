use crate::{InitializeGame, GAME_AUTHORITY_PUBKEY, SUBMISSION_DEADLINE_TIMESTAMP};
use anchor_lang::prelude::*;
use std::str::FromStr;

pub fn initialize_game(ctx: Context<InitializeGame>) -> Result<()> {
    msg!("Initializing game...");
    let game = &mut ctx.accounts.game;
    game.authority =
        Pubkey::from_str(GAME_AUTHORITY_PUBKEY).map_err(|_| ProgramError::InvalidArgument)?;
    game.result = None;
    game.is_open_for_bets = true;
    game.is_open_for_reveals = false;
    game.bet_count = 0;
    game.total_player_pot = 0;
    game.bump = ctx.bumps.game;
    game.treasury_bump = ctx.bumps.game_treasury;

    // Set hardcoded submission deadline
    game.submission_deadline = Some(SUBMISSION_DEADLINE_TIMESTAMP);
    game.reveal_deadline = None; // Reveal deadline set when result is submitted
    game.final_claim_deadline = None;

    msg!(
        "Game initialized with hardcoded authority: {}. Hardcoded Submission deadline: {}",
        game.authority,
        SUBMISSION_DEADLINE_TIMESTAMP
    );
    Ok(())
}
