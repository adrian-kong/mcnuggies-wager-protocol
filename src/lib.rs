use anchor_lang::prelude::*;
use anchor_lang::solana_program::{keccak, program::invoke_signed, system_instruction};

declare_id!("8JD6JtkBzExbDZkpQBvowXngMr9tDqLwf5sGGjBacwK8");

// --- Payout Curve Constants ---
// Multiplier M(x) = 3.9 * exp(-0.1 * x) + 0.1 where x = result - guess
// We use a scaling factor to represent the multiplier as an integer
const PAYOUT_SCALE: u64 = 1_000_000; // 6 decimal places precision

// Precomputed lookup table for M(x) * PAYOUT_SCALE for x = 0 to 100
// Calculated using `round((3.9 * exp(-0.1 * x) + 0.1) * 1_000_000)`
const PAYOUT_MULTIPLIER_LUT: [u64; 101] = [
    4000000, 3628864, 3292869, 2988116, 2711062, 2458514, 2227649, 2016017, 1821502, 1642314,
    1476911, 1323951, 1182270, 1050856, 928820, 815392, 709907, 611791, 520561, 435792, 357123,
    284187, 216667, 154202, 096471, 043178, 199314, 159676, 123980, 109956, 104466, 101647, 100607,
    100224, 100083, 100031, 100011, 100004, 100002, 100001, 100000, 100000, 100000, 100000, 100000,
    100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000,
    100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000,
    100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000,
    100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000,
    100000, 100000, 100000, 100000, 100000, 100000, 100000, 100000,
];

#[program]
pub mod nug_wager_protocol {
    use super::*;

    pub fn initialize_game(ctx: Context<InitializeGame>, authority: Pubkey) -> Result<()> {
        let game = &mut ctx.accounts.game;
        game.authority = authority;
        game.result = None;
        game.is_open_for_bets = true;
        game.is_open_for_reveals = false;
        game.bet_count = 0;
        game.total_pot = 0;
        game.bump = ctx.bumps.game;
        game.treasury_bump = ctx.bumps.game_treasury;
        msg!("Game initialized by authority: {}", authority);
        Ok(())
    }

    // Player commits a hash of their bet, salt, and the bet amount
    pub fn commit_bet(ctx: Context<CommitBet>, commitment: [u8; 32], amount: u64) -> Result<()> {
        let game = &mut ctx.accounts.game;
        require!(game.is_open_for_bets, GameError::BettingClosed);
        require!(game.result.is_none(), GameError::ResultAlreadySubmitted);
        require!(amount > 0, GameError::InvalidBetAmount); // Bet must be positive

        // Transfer funds from player to game treasury PDA
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
            &[], // No signer seeds needed for player's transfer
        )?;

        // Store commitment details
        let bet_commitment = &mut ctx.accounts.bet_commitment;
        bet_commitment.player = *ctx.accounts.player.key;
        bet_commitment.commitment = commitment;
        bet_commitment.game = *game.to_account_info().key;
        bet_commitment.amount = amount; // Store the bet amount

        // Update game state
        game.bet_count = game.bet_count.checked_add(1).ok_or(GameError::Overflow)?;
        game.total_pot = game
            .total_pot
            .checked_add(amount)
            .ok_or(GameError::Overflow)?;

        msg!(
            "Bet committed by player: {} for amount: {}",
            bet_commitment.player,
            amount
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
        require!(result <= 100, GameError::InvalidBetValue); // Ensure result is within range

        game.result = Some(result);
        game.is_open_for_bets = false; // Stop new bets
        game.is_open_for_reveals = true; // Allow reveals

        msg!(
            "Result {} submitted by authority: {}",
            result,
            game.authority
        );
        Ok(())
    }

    // Player reveals their bet and salt
    pub fn reveal_bet(ctx: Context<RevealBet>, bet_value: u8, salt: u64) -> Result<()> {
        let game = &ctx.accounts.game;
        require!(game.is_open_for_reveals, GameError::RevealClosed);
        require!(game.result.is_some(), GameError::ResultNotSubmitted);
        require!(bet_value <= 100, GameError::InvalidBetValue); // Ensure bet is within range

        let commitment_account = &ctx.accounts.bet_commitment;
        let player = *ctx.accounts.player.key;
        require!(
            commitment_account.player == player,
            GameError::InvalidPlayer
        );

        // Verify the commitment hash
        let mut hasher = keccak::Hasher::default();
        hasher.hash(&bet_value.to_le_bytes());
        hasher.hash(&salt.to_le_bytes());
        let calculated_commitment = hasher.result().to_bytes();

        require!(
            calculated_commitment == commitment_account.commitment,
            GameError::CommitmentMismatch
        );

        // Store the revealed bet information
        let revealed_bet = &mut ctx.accounts.revealed_bet;
        revealed_bet.player = player;
        revealed_bet.bet_value = bet_value;
        revealed_bet.salt = salt;
        revealed_bet.amount = commitment_account.amount; // Copy bet amount
        revealed_bet.claimed = false;
        revealed_bet.game = *game.to_account_info().key;

        msg!(
            "Bet revealed by player: {} (Bet: {}, Amount: {})",
            player,
            bet_value,
            revealed_bet.amount
        );

        // Optional: Close the commitment account here if desired to reclaim rent
        // ... (Closing logic would go here) ...

        Ok(())
    }

    // Player claims their reward based on the payout curve
    pub fn claim_reward(ctx: Context<ClaimReward>) -> Result<()> {
        let game = &ctx.accounts.game;
        let revealed_bet = &mut ctx.accounts.revealed_bet;

        require!(
            revealed_bet.player == *ctx.accounts.player.key,
            GameError::InvalidPlayer
        );
        require!(!revealed_bet.claimed, GameError::AlreadyClaimed);
        require!(game.result.is_some(), GameError::ResultNotSubmitted); // Ensure result exists

        let true_result = game.result.unwrap();
        let guessed_value = revealed_bet.bet_value;
        let bet_amount = revealed_bet.amount;

        let mut payout_amount: u64 = 0;

        // Payout only if guess <= result (n >= g)
        if guessed_value <= true_result {
            let difference = (true_result - guessed_value) as usize;

            // Ensure difference is within LUT bounds (should be due to 0-100 range)
            if difference < PAYOUT_MULTIPLIER_LUT.len() {
                let scaled_multiplier = PAYOUT_MULTIPLIER_LUT[difference];

                // Calculate payout: (bet_amount * scaled_multiplier) / scale
                // Use u128 for intermediate calculation to prevent overflow
                payout_amount = ((bet_amount as u128 * scaled_multiplier as u128)
                    / (PAYOUT_SCALE as u128)) as u64;

                msg!(
                    "Player {} qualifies for payout. Diff: {}, Multiplier (scaled): {}, Bet: {}, Payout: {}",
                    revealed_bet.player, difference, scaled_multiplier, bet_amount, payout_amount
                );
            } else {
                // This case should technically not be reachable if inputs are validated 0-100
                msg!("Error: Difference index out of bounds.");
                // Decide how to handle: error out or just payout 0? Let's payout 0.
                payout_amount = 0;
            }
        } else {
            msg!(
                "Player {} does not qualify for payout (Guess {} > Result {})",
                revealed_bet.player,
                guessed_value,
                true_result
            );
            payout_amount = 0; // Explicitly set to 0 if guess > result
        }

        if payout_amount > 0 {
            // Check if treasury has enough funds
            let treasury_balance = ctx.accounts.game_treasury.to_account_info().lamports();
            require!(
                treasury_balance >= payout_amount,
                GameError::InsufficientTreasuryFunds
            );

            // Transfer payout from treasury PDA to player
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

            msg!(
                "Transferred payout {} to player {}",
                payout_amount,
                revealed_bet.player
            );
        }

        revealed_bet.claimed = true;
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
    pub total_pot: u64,    // Total lamports wagered in the game
    pub bump: u8,          // Bump for the Game PDA itself (if seeded)
    pub treasury_bump: u8, // Bump for the Treasury PDA
}

const DISCRIMINATOR_LENGTH: usize = 8;
const PUBKEY_LENGTH: usize = 32;
const OPTION_FLAG_LENGTH: usize = 1;
const U8_LENGTH: usize = 1;
const BOOL_LENGTH: usize = 1;
const U64_LENGTH: usize = 8;
const COMMITMENT_LENGTH: usize = 32; // Keccak256 hash

impl Game {
    const LEN: usize = DISCRIMINATOR_LENGTH
        + PUBKEY_LENGTH     // authority
        + OPTION_FLAG_LENGTH + U8_LENGTH // result
        + BOOL_LENGTH       // is_open_for_bets
        + BOOL_LENGTH       // is_open_for_reveals
        + U64_LENGTH        // bet_count
        + U64_LENGTH        // total_pot
        + U8_LENGTH         // bump
        + U8_LENGTH; // treasury_bump
}

#[account]
#[derive(Default)]
pub struct BetCommitment {
    pub player: Pubkey,
    pub commitment: [u8; 32], // Hash(bet_value, salt)
    pub game: Pubkey,         // Reference to the game
    pub amount: u64,          // Amount wagered by the player
}

impl BetCommitment {
    const LEN: usize = DISCRIMINATOR_LENGTH
        + PUBKEY_LENGTH      // player
        + COMMITMENT_LENGTH  // commitment
        + PUBKEY_LENGTH      // game
        + U64_LENGTH; // amount
}

#[account]
#[derive(Default)]
pub struct RevealedBet {
    pub player: Pubkey,
    pub bet_value: u8,
    pub salt: u64,
    // pub is_winner: bool, // Removed, payout logic determines winner
    pub amount: u64, // Amount wagered by the player
    pub claimed: bool,
    pub game: Pubkey, // Reference to the game
}

impl RevealedBet {
    const LEN: usize = DISCRIMINATOR_LENGTH
        + PUBKEY_LENGTH     // player
        + U8_LENGTH         // bet_value
        + U64_LENGTH        // salt
        + U64_LENGTH        // amount (replaced is_winner)
        + BOOL_LENGTH       // claimed
        + PUBKEY_LENGTH; // game
}

// --- Context Structs ---

#[derive(Accounts)]
pub struct InitializeGame<'info> {
    #[account(
        init,
        payer = payer,
        space = Game::LEN,
        seeds = [b"game", payer.key().as_ref()], // Seed with initializer/authority
        bump
    )]
    pub game: Account<'info, Game>,
    /// CHECK: The treasury PDA is initialized implicitly by the system program during the first transfer to it. We just need its address defined by seeds.
    #[account(
        mut,
        seeds = [b"treasury", game.key().as_ref()],
        bump
    )]
    pub game_treasury: UncheckedAccount<'info>,
    #[account(mut)]
    pub payer: Signer<'info>, // The authority usually pays for initialization
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
#[instruction(commitment: [u8; 32], amount: u64)] // Make amount accessible for seed calc if needed later
pub struct CommitBet<'info> {
    // No longer needs authority check, players commit freely
    #[account(mut)]
    pub game: Account<'info, Game>,
    #[account(
        init,
        payer = player,
        space = BetCommitment::LEN,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump
    )]
    pub bet_commitment: Account<'info, BetCommitment>,
    /// CHECK: Treasury PDA needs to be mutable to receive funds. Address validated by seeds.
    #[account(
        mut,
        seeds = [b"treasury", game.key().as_ref()],
        bump = game.treasury_bump // Use bump stored in game account
    )]
    pub game_treasury: UncheckedAccount<'info>,
    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct SubmitResult<'info> {
    #[account(mut, has_one = authority @ GameError::InvalidAuthority)]
    pub game: Account<'info, Game>,
    pub authority: Signer<'info>,
}

#[derive(Accounts)]
#[instruction(bet_value: u8, salt: u64)] // Make params accessible for seed calc if needed
pub struct RevealBet<'info> {
    #[account(
        // No mut needed on game unless we modify it during reveal
        seeds = [b"game", game.authority.as_ref()], // Reconstruct seeds if needed
        bump = game.bump
    )]
    pub game: Account<'info, Game>,

    #[account(
        // No mut needed unless closing
        // Close constraint removed for now
        constraint = bet_commitment.player == player.key() @ GameError::InvalidPlayer,
        constraint = bet_commitment.game == game.key() @ GameError::InvalidGameReference,
        seeds = [b"commitment", game.key().as_ref(), player.key().as_ref()],
        bump,
    )]
    pub bet_commitment: Account<'info, BetCommitment>,

    #[account(
        init, // Initialize the revealed bet account upon successful reveal
        payer = player,
        space = RevealedBet::LEN,
        seeds = [b"revealed", game.key().as_ref(), player.key().as_ref()],
        bump
    )]
    pub revealed_bet: Account<'info, RevealedBet>,

    #[account(mut)]
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>,
}

#[derive(Accounts)]
pub struct ClaimReward<'info> {
    #[account(
        // No mut needed on game unless we modify it during claim (like decrementing pot)
        seeds = [b"game", game.authority.as_ref()], // Use authority to find game PDA
        bump = game.bump,
    )]
    pub game: Account<'info, Game>,

    #[account(
        mut, // Need mut to set claimed = true
        constraint = revealed_bet.player == player.key() @ GameError::InvalidPlayer,
        constraint = revealed_bet.game == game.key() @ GameError::InvalidGameReference,
        seeds = [b"revealed", game.key().as_ref(), player.key().as_ref()],
        bump
    )]
    pub revealed_bet: Account<'info, RevealedBet>,

    /// CHECK: Treasury PDA needs to be mutable to send funds. Address validated by seeds.
    #[account(
        mut,
        seeds = [b"treasury", game.key().as_ref()],
        bump = game.treasury_bump // Use bump stored in game account
    )]
    pub game_treasury: UncheckedAccount<'info>,

    #[account(mut)] // Player needs to sign to claim and receive funds
    pub player: Signer<'info>,
    pub system_program: Program<'info, System>, // Needed for CPI transfer
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
    #[msg("Cannot reveal bets until the result is submitted.")]
    RevealClosed,
    #[msg("The result has not been submitted yet.")]
    ResultNotSubmitted,
    #[msg("The revealed bet and salt do not match the commitment hash.")]
    CommitmentMismatch,
    #[msg("The signer is not the player associated with this bet.")]
    InvalidPlayer,
    // #[msg("This player is not a winner.")] // Removed, payout logic handles this
    // NotAWinner,
    #[msg("The reward has already been claimed.")]
    AlreadyClaimed,
    #[msg("Account references the wrong game.")]
    InvalidGameReference,
    #[msg("Calculation overflow.")]
    Overflow,
    #[msg("Bet amount must be greater than zero.")]
    InvalidBetAmount,
    #[msg("Insufficient funds in the game treasury for payout.")]
    InsufficientTreasuryFunds,
}
