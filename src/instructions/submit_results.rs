use crate::GameError;
use crate::SubmitResult;
use crate::REVEAL_DEADLINE_TIMESTAMP;
use anchor_lang::prelude::*;

pub fn submit_results(ctx: Context<SubmitResult>, result: u8) -> Result<()> {
    require!(result <= 100, GameError::InvalidBetValue);
    let game = &mut ctx.accounts.game;
    game.result = Some(result);
    game.is_open_for_bets = false;
    game.is_open_for_reveals = true;
    game.reveal_deadline = Some(REVEAL_DEADLINE_TIMESTAMP); // Set hardcoded reveal deadline
    msg!(
        "Result {} submitted by authority: {}. Hardcoded Reveal deadline: {}",
        result,
        game.authority,
        REVEAL_DEADLINE_TIMESTAMP
    );
    Ok(())
}
