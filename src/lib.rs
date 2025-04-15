use anchor_lang::prelude::*;
use anchor_lang::solana_program::sysvar::clock::Clock;
use anchor_lang::solana_program::{keccak, program::invoke_signed, system_instruction};
use std::str::FromStr;

declare_id!("8JD6JtkBzExbDZkpQBvowXngMr9tDqLwf5sGGjBacwK8");

// --- Hardcoded Constants ---
const GLOBAL_GAME_SEED: &[u8] = b"ADRIAN_NUGGETS_MINECRAFT_MOVIE";
const GAME_AUTHORITY_PUBKEY: &str = "JDUcdJdTH8j352LvXhWbDKPb7WzTWH8VkfwXeBX2NT7U";
const SUBMISSION_DEADLINE_TIMESTAMP: i64 = 1745208000; // 21st April 2025 4 AM GMT (or 2 PM AEDT)
const REVEAL_DEADLINE_TIMESTAMP: i64 = 1745812800; // 28th April 2025 4 AM GMT (or 2 PM AEDT)

// --- Payout Curve Constants ---
// Multiplier M(x) = 3.9 * exp(-0.1 * x) + 0.1 where x = result - guess
// We use a scaling factor to represent the multiplier as an integer
const PAYOUT_SCALE: u64 = 1_000_000; // 6 decimal places precision

// Precomputed lookup table for M(x) * PAYOUT_SCALE for x = 0 to 100
// Calculated using `round((3.9 * exp(-0.14 * x) + 0.1) * 1_000_000)`
const PAYOUT_MULTIPLIER_LUT: [u64; 101] = [
    4_000_000, 3_490_497, 3_047_557, 2_662_483, 2_327_715, 2_036_683, 1_783_671, 1_563_713,
    1_372_491, 1_206_251, 1_061_728, 936_086, 826_859, 731_900, 649_348, 577_580, 515_188, 460_947,
    413_792, 372_798, 337_159, 306_176, 279_241, 255_825, 235_468, 217_770, 202_384, 189_008,
    177_380, 167_271, 158_483, 150_842, 144_200, 138_426, 133_406, 129_042, 125_248, 121_949,
    119_082, 116_589, 114_422, 112_538, 110_900, 109_476, 108_238, 107_162, 106_226, 105_413,
    104_705, 104_091, 103_556, 103_092, 102_688, 102_337, 102_031, 101_766, 101_535, 101_335,
    101_160, 101_009, 100_877, 100_762, 100_663, 100_576, 100_501, 100_435, 100_379, 100_329,
    100_286, 100_249, 100_216, 100_188, 100_163, 100_142, 100_124, 100_107, 100_093, 100_081,
    100_071, 100_061, 100_053, 100_046, 100_040, 100_035, 100_030, 100_026, 100_023, 100_020,
    100_017, 100_015, 100_013, 100_011, 100_010, 100_009, 100_008, 100_007, 100_006, 100_005,
    100_004, 100_004, 100_003,
];

#[program]
pub mod nug_wager_protocol {
    use anchor_lang::solana_program::native_token::LAMPORTS_PER_SOL;

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
        // limit bet range to 0 to 1 sol
        require!(amount > 0, GameError::InvalidBetAmount);
        require!(amount <= LAMPORTS_PER_SOL, GameError::InvalidBetAmount);

        let bet_commitment = &mut ctx.accounts.bet_commitment;
        require!(
            bet_commitment.player != *ctx.accounts.player.key,
            GameError::PlayerAlreadyCommitted
        );

        // Check submission deadline using Clock sysvar
        let current_timestamp = ctx.accounts.clock.unix_timestamp;
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

    // Host (Adrian) submits the final result
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
            ctx.accounts.clock.unix_timestamp < SUBMISSION_DEADLINE_TIMESTAMP,
            GameError::SubmissionPeriodExpired
        );
        // so Adrian can't just put a reveal deadline before the submission deadline to rug everyone
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

        require!(game.is_open_for_reveals, GameError::RevealPeriodClosed);
        let Some(true_result) = game.result else {
            return Err(GameError::ResultNotSubmitted.into());
        };
        require!(bet_value <= 100, GameError::InvalidBetValue);

        let current_timestamp = ctx.accounts.clock.unix_timestamp;
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
        if bet_value <= true_result {
            let bet_amount = commitment_account.amount;
            let difference = (true_result - bet_value) as usize;
            // since we claim 0 <= guessed_bet <= 100 previously, sanity check max difference is 100
            require!(
                difference < PAYOUT_MULTIPLIER_LUT.len(),
                GameError::InvalidBetValue
            );

            let scaled_multiplier = PAYOUT_MULTIPLIER_LUT[difference];
            let payout_amount =
                ((bet_amount as u128 * scaled_multiplier as u128) / (PAYOUT_SCALE as u128)) as u64;
            msg!(
                "Player {} qualifies for payout. Diff: {}, Multiplier (scaled): {}, Bet: {}, Payout: {}",
                player, difference, scaled_multiplier, bet_amount, payout_amount
            );
            if payout_amount > 0 {
                let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
                require!(
                    treasury_balance >= payout_amount,
                    GameError::InsufficientTreasuryFunds
                );

                let game_key = game.key();
                let seeds = &[
                    b"treasury".as_ref(),
                    game_key.as_ref(),
                    &[game.treasury_bump],
                ];
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
            } else {
                msg!("No payout for player {}, payout: {}", player, payout_amount);
            }
        } else {
            msg!(
                "Player {} lost (guessed {} > result {})",
                player,
                bet_value,
                true_result
            );
        }

        msg!(
            "Closing commitment account for player {}, returning rent.",
            player
        );
        Ok(())
    }

    // --- TIMEOUT INSTRUCTIONS ---

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

        let current_timestamp = ctx.accounts.clock.unix_timestamp;
        let submission_deadline = game.submission_deadline.ok_or(GameError::DeadlineNotSet)?;
        require!(
            current_timestamp >= submission_deadline,
            GameError::SubmissionDeadlineNotReached
        );

        let reclaim_amount = commitment.amount;
        let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
        // woops, casino bankrupt ggs. contact me for payout? guess this really trusts the authority
        // ensure liquidity in treasury is high enough to cover all bets before making your bets!
        require!(
            treasury_balance >= reclaim_amount,
            GameError::InsufficientTreasuryForReclaim
        );
        let game_key = game.key();
        let seeds = &[
            b"treasury".as_ref(),
            game_key.as_ref(),
            &[game.treasury_bump],
        ];
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
        let game_key = game.key();
        let seeds = &[
            b"treasury".as_ref(),
            game_key.as_ref(),
            &[game.treasury_bump],
        ];
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
    // we'll just store this on chain so people can see easily i guess?
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
        seeds = [b"treasury", game.key().as_ref()],
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
        seeds = [b"treasury", game.key().as_ref()],
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
        seeds = [b"treasury", game.key().as_ref()],
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
        seeds = [b"treasury", game.key().as_ref()],
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
        seeds = [b"treasury", game.key().as_ref()],
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
        seeds = [b"treasury", game.key().as_ref()],
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
    #[msg("Player has already committed to this game.")]
    PlayerAlreadyCommitted,

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
