use anchor_lang::prelude::*;
use anchor_lang::solana_program::native_token::LAMPORTS_PER_SOL;
use anchor_lang::solana_program::sysvar::clock::Clock;
use anchor_lang::solana_program::{keccak, program::invoke_signed, system_instruction};
use anchor_lang::solana_program::sysvar;
use std::str::FromStr;

declare_id!("8JD6JtkBzExbDZkpQBvowXngMr9tDqLwf5sGGjBacwK8");

// --- Hardcoded Constants ---
const GLOBAL_GAME_SEED: &[u8] = b"ADRIAN_NUGGETS_MINECRAFT_MOVIE";
const GAME_AUTHORITY_PUBKEY: &str = "JDUcdJdTH8j352LvXhWbDKPb7WzTWH8VkfwXeBX2NT7U";

// ENSURE THESE ARE SET BEFORE GOING LIVE, IT SHOULD BE IN ORDER
const SUBMISSION_DEADLINE_TIMESTAMP: i64 = 1745208000; // 21st April 2025 4 AM GMT (or 2 PM AEDT)
const REVEAL_DEADLINE_TIMESTAMP: i64 = 1745812800; // 28th April 2025 4 AM GMT (or 2 PM AEDT)
const FINAL_CLAIM_DEADLINE_TIMESTAMP: i64 = 1746408000; // 5th May 2025 4 AM GMT (or 2 PM AEDT)

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

    // Player commits a hash of their bet, salt, and the bet amount
    pub fn commit_bet(ctx: Context<CommitBet>, commitment: [u8; 32], amount: u64) -> Result<()> {
        // limit bet range to 0 to 1 sol
        require!(0 < amount && amount <= LAMPORTS_PER_SOL, GameError::InvalidBetAmount);
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

    // Host (Adrian) submits the final result
    pub fn submit_result(ctx: Context<SubmitResult>, result: u8) -> Result<()> {
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

    // Player reveals their bet, salt and claims reward in one step
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
        require!(hashed == commitment_account.commitment, GameError::CommitmentMismatch);
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
            msg!(
                "Player lost, no payout for player {}. Bet marked as settled.",
                player
            );
            // player is exiting the pot, decrementing the initial staked bet from total player pot
            game.total_player_pot = game
                .total_player_pot
                .checked_sub(bet_amount)
                .ok_or(GameError::TotalPayoutPotDesynced)?;
            msg!("Closing commitment account and returning rent to player.");
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
            player, difference, scaled_multiplier, bet_amount, payout_amount
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
            msg!("Closing commitment account and returning rent to player.");
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
            // set the final claim deadline so player can reclaim their initial stake later if host does not fund
            // we don't handle potentially splittng treasury amongst players as thats a bit complicated. lets assume im at least that trustworthy
            game.final_claim_deadline = Some(FINAL_CLAIM_DEADLINE_TIMESTAMP);
            msg!("Host liquidity insufficient for payout. Player can use withdraw_unpaid_bet to reclaim their bet.");
            return Err(GameError::InsufficientHostLiquidity.into());
        }
        // updating total_player_pot to reflect the payout, decrementing initial stake so remaining comes out of host's liquidity
        game.total_player_pot = game
            .total_player_pot
            .checked_sub(bet_amount)
            .ok_or(GameError::TotalPayoutPotDesynced)?;

        // perform payout
        msg!(
            "Implicit host liquidity sufficient ({} >= {}). Proceeding with transfer.",
            host_liquidity,
            payout_amount
        );
        let game_key = game.key();
        let seeds = &[
            b"treasury".as_ref(),
            game_key.as_ref(),
            &[game.treasury_bump],
        ];
        invoke_signed(
            &system_instruction::transfer(ctx.accounts.game_treasury.key, &player, payout_amount),
            &[
                ctx.accounts.game_treasury.to_account_info(),
                ctx.accounts.player.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[&seeds[..]],
        )?;
        msg!("Transferred payout {} to player {}. Bet marked as settled. Player should call CleanupBetCommitment to reclaim rent.", payout_amount, player);
        msg!("Closing commitment account and returning rent to player.");
        Ok(())
    }

    // Player withdraws original bet if host had INSUFFICIENT LIQUIDITY for payout AFTER REVEAL DEADLINE BEFORE FINAL CLAIM DEADLINE
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
        let game_key = game.key();
        let seeds = &[
            b"treasury".as_ref(),
            game_key.as_ref(),
            &[game.treasury_bump],
        ];
        invoke_signed(
            &system_instruction::transfer(ctx.accounts.game_treasury.key, &player, reclaim_amount),
            &[
                ctx.accounts.game_treasury.to_account_info(),
                ctx.accounts.player.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            &[&seeds[..]],
        )?;

        msg!(
            "Host lacked liquidity. Withdrew original bet {} lamports for player {}.",
            reclaim_amount,
            player
        );
        msg!("Closing commitment account and returning rent to player.");
        Ok(())
    }

    // --- TIMEOUT INSTRUCTIONS ---

    // Player reclaims their original bet if authority missed submission deadline
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

    // Authority claims after reveal deadline, or if someone flagged illiquidity then after final claim deadline 
    // (as this period between will allow players to claim back their initial stake preventing rug)
    pub fn claim_remaining_treasury(ctx: Context<ClaimRemainingTreasury>) -> Result<()> {
        let game = &mut ctx.accounts.game;
        // provided authority from the signer
        let authority = *ctx.accounts.authority.key;
        let game_treasury = &ctx.accounts.game_treasury;
        let treasury_balance = game_treasury.to_account_info().lamports();
        require!(treasury_balance > 0, GameError::TreasuryIsEmpty);
        let game_key = game.key();
        let seeds = &[
            b"treasury".as_ref(),
            game_key.as_ref(),
            &[game.treasury_bump],
        ];
        let signer_seeds = &[&seeds[..]];
        invoke_signed(
            &system_instruction::transfer(game_treasury.key, &game.authority, treasury_balance),
            &[
                game_treasury.to_account_info(),
                ctx.accounts.authority.to_account_info(),
                ctx.accounts.system_program.to_account_info(),
            ],
            signer_seeds,
        )?;
        msg!(
            "Reveal deadline passed. Claimed implicit host liquidity {} lamports from treasury for authority {}. Remaining player pot obligation: {}.",
            treasury_balance,
            authority,
            game.total_player_pot // Log remaining player funds obligation
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
    #[account(seeds = [b"treasury", game.key().as_ref()], bump)]
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
    #[account(seeds = [b"treasury", game.key().as_ref()], bump = game.treasury_bump)]
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
        constraint = Some(clock.unix_timestamp) < game.reveal_deadline @ GameError::RevealDeadlineNotReached,
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
        has_one = authority @ GameError::InvalidAuthority,
        constraint = game.is_open_for_reveals @ GameError::RevealPeriodClosed,
        constraint = game.submission_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) < game.reveal_deadline @ GameError::RevealDeadlineNotReached,
        constraint = game.total_player_pot >= bet_commitment.amount @ GameError::InsufficientPlayerPot,
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
    #[account(
        mut,
        seeds = [b"treasury", game.key().as_ref()],
        bump = game.treasury_bump
    )]
    pub game_treasury: SystemAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    #[account(mut)]
    pub authority: Signer<'info>,
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
        close = player,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump,
        constraint = bet_commitment.player == player.key() @ GameError::InvalidPlayerForCommitment,
        constraint = bet_commitment.game == game.key() @ GameError::InvalidGameReference,
        constraint = bet_commitment.attempted_reveal @ GameError::BetAlreadySettled,
        constraint = game.reveal_deadline.is_some() @ GameError::DeadlineNotSet,
        constraint = Some(clock.unix_timestamp) > game.reveal_deadline @ GameError::WithdrawPeriodNotReached,
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
    #[account(address = sysvar::clock::ID)]
    pub clock: Sysvar<'info, Clock>,
}


#[derive(Accounts)]
pub struct ClaimRemainingTreasury<'info> {
    #[account(
        mut,
        close = authority, // Game account is NOT closed here
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
    #[msg("Treasury is empty, nothing to claim.")]
    TreasuryIsEmpty,
}
