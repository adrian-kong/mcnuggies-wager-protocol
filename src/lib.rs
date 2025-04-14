use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::clock::Clock;
use anchor_lang::solana_program::{keccak, program::invoke_signed, system_instruction};
use std::str::FromStr;

declare_id!("8JD6JtkBzExbDZkpQBvowXngMr9tDqLwf5sGGjBacwK8");

// --- Hardcoded Constants ---
const GLOBAL_GAME_SEED: &[u8] = b"GLOBAL_GAME_SINGLETON";
const GAME_AUTHORITY_PUBKEY: &str = "JDUcdJdTH8j352LvXhWbDKPb7WzTWH8VkfwXeBX2NT7U";
const SUBMISSION_DEADLINE_TIMESTAMP: i64 = 1745208000; // 21st April 2025 4 AM GMT (or 2 PM AEDT)
const REVEAL_DEADLINE_TIMESTAMP: i64 = 1745812800; // 28th April 2025 4 AM GMT (or 2 PM AEDT)

// --- Payout Curve Constants ---
// Multiplier M(x) = 3.9 * exp(-0.1 * x) + 0.1 where x = result - guess
// We use a scaling factor to represent the multiplier as an integer
const PAYOUT_SCALE: u64 = 1_000_000; // 6 decimal places precision

// Precomputed lookup table for M(x) * PAYOUT_SCALE for x = 0 to 100
// Calculated using `round((3.9 * exp(-0.1 * x) + 0.1) * 1_000_000)`
const PAYOUT_MULTIPLIER_LUT: [u64; 101] = [
    4_000_000, 3_628_864, 3_292_869, 2_988_116, 2_711_062, 2_458_514, 2_227_649, 2_016_017,
    1_821_502, 1_642_314, 1_476_911, 1_323_951, 1_182_270, 1_050_856, 928_820, 815_392, 709_907,
    611_791, 520_561, 435_792, 357_123, 284_187, 216_667, 154_202, 96_471, 43_178, 19_931, 15_967,
    12_398, 10_995, 10_446, 10_164, 10_060, 10_022, 10_008, 10_003, 10_001, 10_000, 10_000, 10_000,
    10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000,
    10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000,
    10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000,
    10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000,
    10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000, 10_000,
    10_000,
];

#[program]
pub mod nug_wager_protocol {
    use super::*;

    pub fn initialize_game(ctx: Context<InitializeGame>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        game.authority =
            Pubkey::from_str(GAME_AUTHORITY_PUBKEY).map_err(|_| ProgramError::InvalidArgument)?;
        game.result = None;
        game.is_open_for_bets = true;
        game.is_open_for_reveals = false;
        game.bet_count = 0;
        game.total_pot = 0;
        game.bump = ctx.bumps.game;
        game.treasury_bump = ctx.bumps.game_treasury;

        // Set hardcoded submission deadline
        game.submission_deadline = Some(SUBMISSION_DEADLINE_TIMESTAMP);
        game.reveal_deadline = None; // Reveal deadline set when result is submitted

        msg!(
            "Game initialized with hardcoded authority: {}. Hardcoded Submission deadline: {}",
            game.authority,
            SUBMISSION_DEADLINE_TIMESTAMP
        );
        Ok(())
    }

    // Player commits a hash of their bet, salt, and the bet amount
    pub fn commit_bet(ctx: Context<CommitBet>, commitment: [u8; 32], amount: u64) -> Result<()> {
        let game = &mut ctx.accounts.game;
        require!(game.is_open_for_bets, GameError::BettingClosed);
        require!(game.result.is_none(), GameError::ResultAlreadySubmitted);
        require!(amount > 0, GameError::InvalidBetAmount); // Bet must be positive

        // Check submission deadline using Clock sysvar
        let current_timestamp = Clock::get()?.unix_timestamp;
        let submission_deadline = game.submission_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            current_timestamp < submission_deadline,
            GameError::SubmissionPeriodExpired
        );

        // --- Rest of the commit logic ---
        let transfer_instruction = system_instruction::transfer(
            ctx.accounts.player.key,
            ctx.accounts.game_treasury.key,
            amount,
        );
        invoke_signed(
            &transfer_instruction,
            &[
                ctx.accounts.player.to_account_info(),
                ctx.accounts.game_treasury.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[],
        )?;

        let bet_commitment = &mut ctx.accounts.bet_commitment;
        bet_commitment.player = *ctx.accounts.player.key;
        bet_commitment.commitment = commitment;
        bet_commitment.game = *game.to_account_info().key;
        bet_commitment.amount = amount;

        game.bet_count = game.bet_count.checked_add(1).ok_or(GameError::Overflow)?;
        game.total_pot = game
            .total_pot
            .checked_add(amount)
            .ok_or(GameError::Overflow)?;

        msg!(
            "Bet committed by player: {} for amount: {}",
            bet_commitment.player,
            amount,
        );
        Ok(())
    }

    // Authority submits the final result
    pub fn submit_result(ctx: Context<SubmitResult>, result: u8) -> Result<()> {
        let game = &mut ctx.accounts.game;

        require!(
            game.authority == *ctx.accounts.authority.key,
            GameError::InvalidAuthority
        );
        require!(game.result.is_none(), GameError::ResultAlreadySubmitted);
        require!(result <= 100, GameError::InvalidBetValue);

        // Check if reveal deadline is after submission deadline (sanity check for constants)
        let submission_deadline = game.submission_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            REVEAL_DEADLINE_TIMESTAMP > submission_deadline,
            GameError::RevealDeadlineMustBeAfterSubmission
        );

        game.result = Some(result);
        game.is_open_for_bets = false;
        game.is_open_for_reveals = true;

        // Set hardcoded reveal deadline
        game.reveal_deadline = Some(REVEAL_DEADLINE_TIMESTAMP);

        msg!(
            "Result {} submitted by authority: {}. Hardcoded Reveal deadline: {}",
            result,
            game.authority,
            REVEAL_DEADLINE_TIMESTAMP
        );
        Ok(())
    }

    // Player reveals their bet, salt and claims reward in one step
    pub fn reveal_and_claim(ctx: Context<RevealAndClaim>, bet_value: u8, salt: u64) -> Result<()> {
        let game = &ctx.accounts.game;
        let commitment_account = &ctx.accounts.bet_commitment;
        let player = *ctx.accounts.player.key;

        // --- Reveal Logic --- //
        require!(game.is_open_for_reveals, GameError::RevealPeriodClosed);
        require!(game.result.is_some(), GameError::ResultNotSubmitted);
        require!(bet_value <= 100, GameError::InvalidBetValue);

        // Check reveal deadline using Clock sysvar
        let current_timestamp = Clock::get()?.unix_timestamp;
        let reveal_deadline = game.reveal_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            current_timestamp < reveal_deadline,
            GameError::RevealPeriodExpired
        );

        require!(
            commitment_account.player == player,
            GameError::InvalidPlayerForCommitment
        );
        require!(
            commitment_account.game == game.key(),
            GameError::InvalidGameReference
        );

        let mut hasher = keccak::Hasher::default();
        hasher.hash(&bet_value.to_le_bytes());
        hasher.hash(&salt.to_le_bytes());
        let calculated_commitment = hasher.result().to_bytes();
        require!(
            calculated_commitment == commitment_account.commitment,
            GameError::CommitmentMismatch
        );

        msg!(
            "Bet reveal verified for player: {} (Bet: {}, Salt: {}, Amount: {})",
            player,
            bet_value,
            salt,
            commitment_account.amount
        );

        // --- Claim Logic --- //
        let true_result = game.result.unwrap();
        let guessed_value = bet_value;
        let bet_amount = commitment_account.amount;

        let mut payout_amount: u64 = 0;
        if guessed_value <= true_result {
            let difference = (true_result - guessed_value) as usize;
            if difference < PAYOUT_MULTIPLIER_LUT.len() {
                let scaled_multiplier = PAYOUT_MULTIPLIER_LUT[difference];
                payout_amount = ((bet_amount as u128 * scaled_multiplier as u128)
                    / (PAYOUT_SCALE as u128)) as u64;
                msg!(
                    "Player {} qualifies for payout. Diff: {}, Multiplier (scaled): {}, Bet: {}, Payout: {}",
                    player, difference, scaled_multiplier, bet_amount, payout_amount
                );
            } else {
                msg!("Error: Difference index out of bounds. Payout set to 0.");
            }
        } else {
            msg!(
                "Player {} does not qualify for payout (Guess {} > Result {})",
                player,
                guessed_value,
                true_result
            );
        }

        if payout_amount > 0 {
            let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
            require!(
                treasury_balance >= payout_amount,
                GameError::InsufficientTreasuryFunds
            );

            let seeds = &[b"treasury".as_ref(), &[game.treasury_bump]];
            let signer_seeds = &[&seeds[..]];
            let transfer_instruction = system_instruction::transfer(
                ctx.accounts.game_treasury.key,
                ctx.accounts.player.key,
                payout_amount,
            );
            invoke_signed(
                &transfer_instruction,
                &[
                    ctx.accounts.game_treasury.to_account_info(),
                    ctx.accounts.player.to_account_info(),
                    ctx.accounts.system_program.to_account_info(),
                ],
                signer_seeds,
            )?;
            msg!("Transferred payout {} to player {}", payout_amount, player);
        }

        msg!(
            "Closing commitment account for player {} and returning rent.",
            player
        );
        Ok(())
    }

    // --- NEW TIMEOUT INSTRUCTIONS ---

    // Player reclaims their original bet if authority missed submission deadline
    pub fn reclaim_bet_on_timeout(ctx: Context<ReclaimBetOnTimeout>) -> Result<()> {
        let game = &ctx.accounts.game;
        let commitment = &ctx.accounts.bet_commitment;
        let player = *ctx.accounts.player.key;

        require!(
            commitment.player == player,
            GameError::InvalidPlayerForCommitment
        );
        require!(
            commitment.game == game.key(),
            GameError::InvalidGameReference
        );
        require!(
            game.result.is_none(),
            GameError::ResultAlreadySubmittedCannotReclaim
        );

        let current_timestamp = Clock::get()?.unix_timestamp;
        let submission_deadline = game.submission_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            current_timestamp >= submission_deadline,
            GameError::SubmissionDeadlineNotReached
        );

        let reclaim_amount = commitment.amount;
        let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
        require!(
            treasury_balance >= reclaim_amount,
            GameError::InsufficientTreasuryForReclaim
        );

        let seeds = &[b"treasury".as_ref(), &[game.treasury_bump]];
        let signer_seeds = &[&seeds[..]];
        invoke_signed(
            &system_instruction::transfer(
                ctx.accounts.game_treasury.key,
                ctx.accounts.player.key,
                reclaim_amount,
            ),
            &[
                ctx.accounts.game_treasury.to_account_info(),
                ctx.accounts.player.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            signer_seeds,
        )?;

        msg!(
            "Authority missed deadline. Reclaimed {} lamports for player {}.",
            reclaim_amount,
            player
        );
        msg!("Closing commitment account and returning rent to player.");
        Ok(())
    }

    // Authority claims the entire remaining treasury balance after reveal deadline
    pub fn claim_remaining_treasury(ctx: Context<ClaimRemainingTreasury>) -> Result<()> {
        let game = &ctx.accounts.game;
        let authority = *ctx.accounts.authority.key;

        require!(game.authority == authority, GameError::InvalidAuthority);

        let current_timestamp = Clock::get()?.unix_timestamp;
        let reveal_deadline = game.reveal_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            current_timestamp >= reveal_deadline,
            GameError::RevealDeadlineNotReached
        );

        let transfer_amount = ctx.accounts.game_treasury.to_account_info().lamports();
        require!(transfer_amount > 0, GameError::TreasuryIsEmpty);
        let seeds = &[b"treasury".as_ref(), &[game.treasury_bump]];
        let signer_seeds = &[&seeds[..]];
        invoke_signed(
            &system_instruction::transfer(
                ctx.accounts.game_treasury.key,
                ctx.accounts.authority.key,
                transfer_amount,
            ),
            &[
                ctx.accounts.game_treasury.to_account_info(),
                ctx.accounts.authority.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            signer_seeds,
        )?;

        msg!(
            "Reveal deadline passed. Claimed remaining {} lamports from treasury for authority {}.",
            transfer_amount,
            authority
        );
        Ok(())
    }

    // Authority cleans up the game account after the reveal deadline
    pub fn cleanup_game(ctx: Context<CleanupGame>) -> Result<()> {
        let game = &ctx.accounts.game;

        // Check reveal deadline using Clock sysvar
        let current_timestamp = Clock::get()?.unix_timestamp;
        let reveal_deadline = game.reveal_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            current_timestamp >= reveal_deadline,
            GameError::CleanupNotAllowedYet
        );

        // Treasury balance check is done via constraints
        msg!(
            "Reveal deadline passed and treasury is empty. Cleaning up game account {} and returning rent to authority {}.",
            game.key(),
            ctx.accounts.authority.key()
        );
        Ok(())
    }
}

// --- Account Structs ---

#[account]
#[derive(Default)]
pub struct Game {
    pub authority: Pubkey,
    pub result: Option<u8>,
    pub is_open_for_bets: bool,
    pub is_open_for_reveals: bool,
    pub bet_count: u64,
    pub total_pot: u64,
    pub bump: u8,
    pub treasury_bump: u8,
    pub submission_deadline: Option<i64>, // Unix timestamp
    pub reveal_deadline: Option<i64>,     // Unix timestamp
}

const DISCRIMINATOR_LENGTH: usize = 8;
const PUBKEY_LENGTH: usize = 32;
const OPTION_FLAG_LENGTH: usize = 1;
const U8_LENGTH: usize = 1;
const BOOL_LENGTH: usize = 1;
const U64_LENGTH: usize = 8;
const I64_LENGTH: usize = 8; // For UnixTimestamp (i64)
const COMMITMENT_LENGTH: usize = 32;

impl Game {
    const LEN: usize = DISCRIMINATOR_LENGTH
        + PUBKEY_LENGTH     // authority
        + OPTION_FLAG_LENGTH + U8_LENGTH // result
        + BOOL_LENGTH       // is_open_for_bets
        + BOOL_LENGTH       // is_open_for_reveals
        + U64_LENGTH        // bet_count
        + U64_LENGTH        // total_pot
        + U8_LENGTH         // bump
        + U8_LENGTH         // treasury_bump
        + OPTION_FLAG_LENGTH + I64_LENGTH // submission_deadline
        + OPTION_FLAG_LENGTH + I64_LENGTH; // reveal_deadline
}

#[account]
#[derive(Default)]
pub struct BetCommitment {
    pub player: Pubkey,
    pub commitment: [u8; 32],
    pub game: Pubkey,
    pub amount: u64,
}

impl BetCommitment {
    const LEN: usize = DISCRIMINATOR_LENGTH
        + PUBKEY_LENGTH      // player
        + COMMITMENT_LENGTH  // commitment
        + PUBKEY_LENGTH      // game
        + U64_LENGTH; // amount
}

// --- Context Structs ---

#[derive(Accounts)]
#[instruction()]
pub struct InitializeGame<'info> {
    #[account(
        init,
        payer = payer,
        space = Game::LEN,
        seeds = [GLOBAL_GAME_SEED],
        bump
    )]
    pub game: Account<'info, Game>,

    /// CHECK: This is a PDA for holding SOL, no data is accessed or deserialized.
    #[account(
        seeds = [b"treasury"],
        bump
    )]
    pub game_treasury: SystemAccount<'info>,

    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(commitment: [u8; 32], amount: u64)]
pub struct CommitBet<'info> {
    #[account(mut, seeds = [GLOBAL_GAME_SEED], bump = game.bump)]
    pub game: Account<'info, Game>,
    #[account(
        init,
        payer = player,
        space = BetCommitment::LEN,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump
    )]
    pub bet_commitment: Account<'info, BetCommitment>,

    /// CHECK: This is a PDA for holding SOL, player deposits bet into this account
    #[account(
        mut,
        seeds = [b"treasury"],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,

    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
#[instruction(result: u8)] // Removed timestamp instruction parameter
pub struct SubmitResult<'info> {
    #[account(
        mut,
        seeds = [GLOBAL_GAME_SEED],
        bump = game.bump,
        has_one = authority @ GameError::InvalidAuthority
    )]
    pub game: Account<'info, Game>,
    pub authority: Signer<'info>,
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
#[instruction(bet_value: u8, salt: u64)]
pub struct RevealAndClaim<'info> {
    #[account(
        seeds = [GLOBAL_GAME_SEED],
        bump = game.bump,
        has_one = authority @ GameError::InvalidAuthority
    )]
    pub game: Account<'info, Game>,
    #[account(
        mut,
        close = player,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump,
        constraint = bet_commitment.player == player.key() @ GameError::InvalidPlayerForCommitment,
        constraint = bet_commitment.game == game.key() @ GameError::InvalidGameReference
    )]
    pub bet_commitment: Account<'info, BetCommitment>,

    /// CHECK: This is a PDA for holding SOL, player withdraws payouts from this account
    #[account(
        mut,
        seeds = [b"treasury", GLOBAL_GAME_SEED],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    /// CHECK: This is authority to reclaim bet
    #[account(mut)] // Authority needed for game PDA derivation check via has_one
    pub authority: UncheckedAccount<'info>,
    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

// --- NEW CONTEXTS FOR TIMEOUTS ---

#[derive(Accounts)]
pub struct ReclaimBetOnTimeout<'info> {
    // Game account needed to check deadline and authority for seeds
    #[account(seeds = [GLOBAL_GAME_SEED], bump = game.bump)]
    pub game: Account<'info, Game>,

    #[account(
        mut,
        close = player, // Return rent to player
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump,
        constraint = bet_commitment.player == player.key() @ GameError::InvalidPlayerForCommitment,
        constraint = bet_commitment.game == game.key() @ GameError::InvalidGameReference,
    )]
    pub bet_commitment: Account<'info, BetCommitment>,
    #[account(
        mut,
        seeds = [b"treasury", GLOBAL_GAME_SEED],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,

    #[account(mut)] // Player signs to reclaim and receive rent/funds
    pub player: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
pub struct ClaimRemainingTreasury<'info> {
    #[account(
        mut,
        close = authority, // Return rent to the authority
        has_one = authority @ GameError::InvalidAuthority,
        seeds = [GLOBAL_GAME_SEED],
        bump = game.bump
    )]
    pub game: Account<'info, Game>,

    #[account(mut)] // Authority signs to trigger cleanup and receive rent
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [b"treasury", GLOBAL_GAME_SEED],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

// --- NEW CONTEXT FOR CLEANUP ---

#[derive(Accounts)]
pub struct CleanupGame<'info> {
    #[account(
        mut,
        close = authority, // Return rent to the authority
        has_one = authority @ GameError::InvalidAuthority,
        seeds = [GLOBAL_GAME_SEED],
        bump = game.bump
    )]
    pub game: Account<'info, Game>,

    /// CHECK: This is a PDA for holding SOL, no data is accessed or deserialized.
    #[account(
        seeds = [b"treasury", GLOBAL_GAME_SEED],
        bump = game.treasury_bump,
        // Ensure treasury is empty before closing the game state
        constraint = game_treasury.lamports() == 0 @ GameError::TreasuryNotEmpty
    )]
    pub game_treasury: SystemAccount<'info>,

    #[account(mut)] // Authority signs to trigger cleanup and receive rent
    pub authority: Signer<'info>,

    pub system_program: Program<'info, System>,
    pub clock: Sysvar<'info, Clock>,
}

// --- Error Enum ---

#[error_code]
pub enum GameError {
    #[msg("Betting is currently closed for this game.")]
    BettingClosed,
    #[msg("The result has already been submitted.")]
    ResultAlreadySubmitted,
    #[msg("Invalid authority for this action.")]
    InvalidAuthority,
    #[msg("Bet value must be between 0 and 100.")]
    InvalidBetValue,
    #[msg("Cannot reveal/claim until the result is submitted.")]
    ResultNotSubmitted,
    #[msg("Reveal period is closed (either generally or via deadline passed).")]
    RevealPeriodClosed,
    #[msg("The revealed bet and salt do not match the commitment hash.")]
    CommitmentMismatch,
    #[msg("The signer is not the player associated with this commitment.")]
    InvalidPlayerForCommitment,
    #[msg("Account references the wrong game.")]
    InvalidGameReference,
    #[msg("Calculation overflow.")]
    Overflow,
    #[msg("Bet amount must be greater than zero.")]
    InvalidBetAmount,
    #[msg("Insufficient funds in the game treasury for payout.")]
    InsufficientTreasuryFunds,

    // New Errors for Timeouts
    #[msg("Betting/submission period has expired.")]
    SubmissionPeriodExpired,
    #[msg("Reveal/claim period has expired.")]
    RevealPeriodExpired,
    #[msg("Submission deadline has not been reached yet.")]
    SubmissionDeadlineNotReached,
    #[msg("Reveal deadline has not been reached yet.")]
    RevealDeadlineNotReached,
    #[msg("Result already submitted, cannot reclaim bet via timeout.")]
    ResultAlreadySubmittedCannotReclaim,
    #[msg("Deadline timestamp not set in game account, cannot check timeout.")]
    DeadlineNotSet,
    #[msg("Insufficient funds in treasury to reclaim bet on timeout.")]
    InsufficientTreasuryForReclaim,
    #[msg("Insufficient funds in treasury to claim forfeited bet on timeout.")]
    InsufficientTreasuryForForfeit,
    #[msg("Treasury is empty, nothing to claim.")]
    TreasuryIsEmpty,
    #[msg("Reveal deadline must be after the submission deadline.")]
    RevealDeadlineMustBeAfterSubmission,
    #[msg("Cleanup cannot be performed until the reveal deadline has passed.")]
    CleanupNotAllowedYet,
    #[msg("Game treasury must be empty before cleaning up the game account.")]
    TreasuryNotEmpty,
}
