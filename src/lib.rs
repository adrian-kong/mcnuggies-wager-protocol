use anchor_lang::prelude::*;
use anchor_lang::solana_program::system_instruction;
use anchor_lang::solana_program::program::invoke_signed;
use anchor_lang::solana_program::sysvar::clock::Clock;
use anchor_lang::solana_program::sysvar;
pub mod instructions;

declare_id!("8JD6JtkBzExbDZkpQBvowXngMr9tDqLwf5sGGjBacwK8");

// --- Hardcoded Constants ---
pub const GLOBAL_GAME_SEED: &[u8] = b"ADRIAN_NUGGETS_MINECRAFT_MOVIE";
pub const GAME_AUTHORITY_PUBKEY: &str = "JDUcdJdTH8j352LvXhWbDKPb7WzTWH8VkfwXeBX2NT7U";

// ENSURE THESE ARE SET BEFORE GOING LIVE, IT SHOULD BE IN ORDER, 
// OTHERWISE THE GAME WILL NOT WORK!!!!
pub const SUBMISSION_DEADLINE_TIMESTAMP: i64 = 1745208000; // 21st April 2025 4 AM GMT (or 2 PM AEDT)
pub const REVEAL_DEADLINE_TIMESTAMP: i64 = 1745812800; // 28th April 2025 4 AM GMT (or 2 PM AEDT)
pub const FINAL_CLAIM_DEADLINE_TIMESTAMP: i64 = 1746408000; // 5th May 2025 4 AM GMT (or 2 PM AEDT)

// --- Payout Curve Constants ---
// Multiplier M(x) = 3.9 * exp(-0.1 * x) + 0.1 where x = result - guess
// We use a scaling factor to represent the multiplier as an integer
pub const PAYOUT_SCALE: u64 = 1_000_000; // 6 decimal places precision

// Precomputed lookup table for M(x) * PAYOUT_SCALE for x = 0 to 100
// Calculated using `round((3.9 * exp(-0.14 * x) + 0.1) * 1_000_000)`
pub const PAYOUT_MULTIPLIER_LUT: [u64; 101] = [
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

pub fn withdraw_from_treasury_to_player<'info>(
    game: &Account<'info,Game>, 
    game_treasury: &SystemAccount<'info>,
    system_program: &Program<'info, System>,
    player: &Signer<'info>, 
    amount: u64, 
) -> Result<()> {
    let game_key = game.key();
    let seeds = &[
        b"treasury".as_ref(),
        game_key.as_ref(),
        &[game.treasury_bump],
    ];
    let signer_seeds = &[&seeds[..]];
    invoke_signed(
        &system_instruction::transfer(game_treasury.key, player.key, amount),
        &[
            game_treasury.to_account_info(),
            player.to_account_info(),
            system_program.to_account_info(),
        ],
        signer_seeds,
    )?;
    Ok(())
}

#[program]
pub mod nug_wager_protocol {
    use super::*;

    pub fn initialize_game(ctx: Context<InitializeGame>) -> Result<()> {
        instructions::initialize_game(ctx)
    }

    // Player commits a hash of their bet, salt, and the bet amount
    pub fn commit_bet(ctx: Context<CommitBet>, commitment: [u8; 32], amount: u64) -> Result<()> {
        instructions::commit_bet(ctx, commitment, amount)
    }

    // Host (Adrian) submits the final result
    pub fn submit_result(ctx: Context<SubmitResult>, result: u8) -> Result<()> {
        instructions::submit_results(ctx, result)
    }

    // Player reveals their bet, salt and claims reward in one step
    pub fn reveal_and_claim(ctx: Context<RevealAndClaim>, bet_value: u8, salt: u64) -> Result<()> {
        instructions::reveal_and_claim(ctx, bet_value, salt)
    }

    // Player withdraws original bet if host had INSUFFICIENT LIQUIDITY for payout AFTER REVEAL DEADLINE BEFORE FINAL CLAIM DEADLINE
    pub fn withdraw_unpaid_bet(ctx: Context<WithdrawUnpaidBet>) -> Result<()> {
        instructions::withdraw_unpaid_bet(ctx)
    }

    // --- TIMEOUT INSTRUCTIONS ---

    // Player reclaims their original bet if authority missed submission deadline
    pub fn reclaim_bet_on_timeout(ctx: Context<ReclaimBetOnTimeout>) -> Result<()> {
        instructions::reclaim_bet_on_timeout(ctx)
    }

    // Authority claims after reveal deadline, or if someone flagged illiquidity then after final claim deadline 
    // (as this period between will allow players to claim back their initial stake preventing rug)
    // This also cleans up game
    pub fn claim_remaining_treasury(ctx: Context<ClaimRemainingTreasury>) -> Result<()> {
        instructions::claim_remaining_treasury(ctx)
    }
}

// --- Account Structs ---

#[account]
#[derive(Default)]
pub struct Game {
    pub authority: Pubkey,
    pub result: Option<u8>,
    // we should use enums but im too far gone
    pub is_open_for_bets: bool,
    pub is_open_for_reveals: bool,
    pub bet_count: u64,
    pub total_player_pot: u64,
    pub bump: u8,
    pub treasury_bump: u8,
    // we'll just store this on chain so people can see easily i guess?
    pub submission_deadline: Option<i64>,  // Unix timestamp
    pub reveal_deadline: Option<i64>,      // Unix timestamp
    pub final_claim_deadline: Option<i64>, // Unix timestamp
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
        + U64_LENGTH        // total_player_pot
        + U8_LENGTH         // bump
        + U8_LENGTH         // treasury_bump
        + OPTION_FLAG_LENGTH + I64_LENGTH // submission_deadline
        + OPTION_FLAG_LENGTH + I64_LENGTH // reveal_deadline
        + OPTION_FLAG_LENGTH + I64_LENGTH; // final_claim_deadline
}

#[account]
#[derive(Default)]
pub struct BetCommitment {
    pub player: Pubkey,
    pub commitment: [u8; 32],
    // not really needed for static game, but we'll keep it for now
    pub game: Pubkey,
    pub amount: u64,

    // keeping track of players who have attempted to reveal their bet and claim their winnings
    // but was unsuccessful due to the host not having enough liquidity.
    // so we can prevent host from rugging them out of their rightful winnings,
    // and they can still reclaim their bet later if host does not fund.
    pub attempted_reveal: bool,
}

impl BetCommitment {
    const LEN: usize = DISCRIMINATOR_LENGTH
        + PUBKEY_LENGTH      // player
        + COMMITMENT_LENGTH  // commitment
        + PUBKEY_LENGTH      // game
        + U64_LENGTH         // amount
        + BOOL_LENGTH; // attempted_reveal
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
    #[account(mut, seeds = [b"treasury", game.key().as_ref()], bump)]
    pub game_treasury: SystemAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(commitment: [u8; 32], amount: u64)]
pub struct CommitBet<'info> {
    #[account(
        mut, 
        seeds = [GLOBAL_GAME_SEED], 
        bump = game.bump, 
        constraint = game.is_open_for_bets && !game.is_open_for_reveals @ GameError::BettingClosed, 
        constraint = game.result.is_none() @ GameError::ResultAlreadySubmitted,
        constraint = game.submission_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) < game.submission_deadline @ GameError::SubmissionDeadlineNotReached,
    )]
    pub game: Account<'info, Game>,
    #[account(
        init,
        payer = player,
        space = BetCommitment::LEN,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump
    )]
    pub bet_commitment: Account<'info, BetCommitment>,
    #[account(mut, seeds = [b"treasury", game.key().as_ref()], bump = game.treasury_bump)]
    pub game_treasury: SystemAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(address = sysvar::clock::ID)]
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
#[instruction(result: u8)] // Removed timestamp instruction parameter
pub struct SubmitResult<'info> {
    #[account(
        mut,
        seeds = [GLOBAL_GAME_SEED],
        bump = game.bump,
        has_one = authority @ GameError::InvalidAuthority,
        constraint = game.is_open_for_bets @ GameError::RevealPeriodClosed,
        constraint = game.result.is_none() @ GameError::ResultAlreadySubmitted,
        constraint = game.submission_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) < game.submission_deadline @ GameError::SubmissionPeriodExpired,
    )]
    pub game: Account<'info, Game>,
    pub authority: Signer<'info>,
    #[account(address = sysvar::clock::ID)]
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
#[instruction(bet_value: u8, salt: u64)]
pub struct RevealAndClaim<'info> {
    #[account(
        mut,
        seeds = [GLOBAL_GAME_SEED],
        bump = game.bump,
        constraint = game.is_open_for_reveals @ GameError::RevealPeriodClosed,
        constraint = game.reveal_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) < game.reveal_deadline @ GameError::RevealDeadlineNotReached,
        constraint = game.total_player_pot >= bet_commitment.amount @ GameError::InsufficientPlayerPot,
    )]
    pub game: Account<'info, Game>,
    #[account(
        mut,
        // close = player,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump,
        constraint = bet_commitment.player == player.key() @ GameError::InvalidPlayerForCommitment,
        constraint = bet_commitment.game == game.key() @ GameError::InvalidGameReference
    )]
    pub bet_commitment: Account<'info, BetCommitment>,
    #[account(mut, seeds = [b"treasury", game.key().as_ref()], bump = game.treasury_bump)]
    pub game_treasury: SystemAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(address = sysvar::clock::ID)]
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
#[instruction(commitment: [u8; 32], amount: u64)]
pub struct WithdrawUnpaidBet<'info> {
    #[account(mut, seeds = [GLOBAL_GAME_SEED], bump = game.bump, constraint = game.is_open_for_reveals @ GameError::RevealPeriodClosed)]
    pub game: Account<'info, Game>,
    #[account(
        mut,
        // close = player,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump,
        constraint = bet_commitment.player == player.key() @ GameError::InvalidPlayerForCommitment,
        constraint = bet_commitment.game == game.key() @ GameError::InvalidGameReference,
        constraint = bet_commitment.attempted_reveal @ GameError::BetAlreadySettled,
        // constraint = game.reveal_deadline.is_some() @ GameError::DeadlineNotSet,
        // constraint = Some(clock.unix_timestamp) > game.reveal_deadline @ GameError::WithdrawPeriodNotReached,
        constraint = game.final_claim_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) < game.final_claim_deadline @ GameError::WithdrawPeriodNotReached,
    )]
    pub bet_commitment: Account<'info, BetCommitment>,
    #[account(
        mut,
        seeds = [b"treasury", game.key().as_ref()],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
    #[account(address = sysvar::clock::ID)]
    pub clock: Sysvar<'info, Clock>,
}

#[derive(Accounts)]
pub struct ReclaimBetOnTimeout<'info> {
    // Game account needed to check deadline and authority for seeds
    #[account(
        mut, 
        seeds = [GLOBAL_GAME_SEED], 
        bump = game.bump, 
        constraint = game.result.is_none() @ GameError::ResultAlreadySubmitted, 
        constraint = game.submission_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) > game.submission_deadline @ GameError::SubmissionPeriodExpired,
    )]
    pub game: Account<'info, Game>,
    #[account(
        mut,
        // close = player, // Return rent to player
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
    #[account(address = sysvar::clock::ID)]
    pub clock: Sysvar<'info, Clock>,
}


#[derive(Accounts)]
pub struct ClaimRemainingTreasury<'info> {
    #[account(
        mut,
        // close = authority,
        has_one = authority @ GameError::InvalidAuthority,
        seeds = [GLOBAL_GAME_SEED],
        constraint = game.result.is_some() @ GameError::ResultAlreadySubmitted,
        constraint = game.reveal_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) >= game.reveal_deadline @ GameError::SubmissionPeriodExpired,
        constraint = game.final_claim_deadline.is_none() || Some(clock.unix_timestamp) >= game.final_claim_deadline @ GameError::TreasuryClaimPeriodNotReached,
        bump = game.bump
    )]
    pub game: Account<'info, Game>,
    #[account(mut)] // Authority signs to trigger claim
    pub authority: Signer<'info>,
    #[account(
        mut,
        seeds = [b"treasury", game.key().as_ref()],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,
    pub system_program: Program<'info, System>,
    #[account(address = sysvar::clock::ID)]
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
    #[msg("Reveal deadline must be after the submission deadline.")]
    RevealDeadlineMustBeAfterSubmission,
    #[msg("Cleanup cannot be performed until the reveal deadline has passed.")]
    CleanupNotAllowedYet,
    #[msg("Host liquidity insufficient to cover payout.")]
    InsufficientHostLiquidity,
    #[msg("Total Payout Pot Desynced??? Some bug must have happened.")]
    TotalPayoutPotDesynced,
    #[msg("Bet has already been settled (paid out, lost, or reclaimed).")]
    BetAlreadySettled,
    #[msg("Withdrawal period (after reveal deadline) not reached.")]
    WithdrawPeriodNotReached,
    #[msg("Treasury claim period not reached.")]
    TreasuryClaimPeriodNotReached,
    #[msg("Player pot is insufficient to cover bet amount.")]
    InsufficientPlayerPot,
}
