use anchor_lang::{prelude::*, AnchorDeserialize, AnchorSerialize};
use anchor_spl::{
    associated_token::AssociatedToken,
    token::{Mint, Token, TokenAccount},
};

use crate::{
    constants::*,
    errors::MMMErrorCode,
    state::Pool,
    util::{
        check_allowlists_for_mint, get_sol_lp_fee, get_sol_referral_fee,
        get_sol_total_price_and_next_price, pay_creator_fees_in_sol, try_close_pool,
    },
};

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct SolFulfillBuyArgs {
    asset_amount: u64,
    min_payment_amount: u64,
}

// FulfillBuy means a seller wants to sell NFT/SFT into the pool
// where the pool has some buyside payment liquidity. Therefore,
// the seller expects a min_payment_amount that goes back to the
// seller's wallet for the asset_amount that the seller wants to sell.
#[derive(Accounts)]
#[instruction(args:SolFulfillBuyArgs)]
pub struct SolFulfillBuy<'info> {
    #[account(mut)]
    pub payer: Signer<'info>,
    /// CHECK: we will check the owner field that matches the pool owner
    #[account(mut)]
    pub owner: UncheckedAccount<'info>,
    pub cosigner: Signer<'info>,
    #[account(mut)]
    /// CHECK: we will check that the referral matches the pool's referral
    pub referral: UncheckedAccount<'info>,
    #[account(
        mut,
        seeds = [POOL_PREFIX.as_bytes(), owner.key().as_ref(), pool.uuid.as_ref()],
        has_one = owner @ MMMErrorCode::InvalidOwner,
        has_one = referral @ MMMErrorCode::InvalidReferral,
        has_one = cosigner @ MMMErrorCode::InvalidCosigner,
        constraint = pool.payment_mint.eq(&Pubkey::default()) @ MMMErrorCode::InvalidPaymentMint,
        constraint = pool.expiry == 0 || pool.expiry > Clock::get().unwrap().unix_timestamp @ MMMErrorCode::Expired,
        bump
    )]
    pub pool: Box<Account<'info, Pool>>,
    /// CHECK: it's a pda, and the private key is owned by the seeds
    #[account(
        mut,
        seeds = [BUYSIDE_SOL_ESCROW_ACCOUNT_PREFIX.as_bytes(), pool.key().as_ref()],
        bump,
    )]
    pub buyside_sol_escrow_account: UncheckedAccount<'info>,
    /// CHECK: we will check the metadata in check_allowlists_for_mint()
    pub asset_metadata: UncheckedAccount<'info>,
    /// CHECK: we will check the master_edtion in check_allowlists_for_mint()
    pub asset_master_edition: UncheckedAccount<'info>,
    /// CHECK: check_allowlists_for_mint
    pub asset_mint: Account<'info, Mint>,
    #[account(
        mut,
        token::mint = asset_mint,
        token::authority = payer,
    )]
    pub payer_asset_account: Box<Account<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = asset_mint,
        associated_token::authority = pool,
    )]
    pub sellside_escrow_token_account: Box<Account<'info, TokenAccount>>,
    #[account(
        init_if_needed,
        payer = payer,
        associated_token::mint = asset_mint,
        associated_token::authority = owner,
    )]
    pub owner_token_account: Box<Account<'info, TokenAccount>>,
    pub system_program: Program<'info, System>,
    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub rent: Sysvar<'info, Rent>,
}

pub fn handler<'info>(
    ctx: Context<'_, '_, '_, 'info, SolFulfillBuy<'info>>,
    args: SolFulfillBuyArgs,
) -> Result<()> {
    let token_program = &ctx.accounts.token_program;
    let system_program = &ctx.accounts.system_program;
    let pool = &mut ctx.accounts.pool;
    let owner = &ctx.accounts.owner;
    let owner_token_account = &ctx.accounts.owner_token_account;
    let referral = &ctx.accounts.referral;

    let payer = &ctx.accounts.payer;
    let payer_asset_account = &ctx.accounts.payer_asset_account;
    let asset_mint = &ctx.accounts.asset_mint;
    let payer_asset_metadata = &ctx.accounts.asset_metadata;
    let asset_master_edition = &ctx.accounts.asset_master_edition;

    let sellside_escrow_token_account = &ctx.accounts.sellside_escrow_token_account;
    let buyside_sol_escrow_account = &ctx.accounts.buyside_sol_escrow_account;
    let pool_key = pool.key();
    let pool_seeds: &[&[&[u8]]] = &[&[
        POOL_PREFIX.as_bytes(),
        pool.owner.as_ref(),
        pool.uuid.as_ref(),
        &[*ctx.bumps.get("pool").unwrap()],
    ]];
    let buyside_sol_escrow_account_seeds: &[&[&[u8]]] = &[&[
        BUYSIDE_SOL_ESCROW_ACCOUNT_PREFIX.as_bytes(),
        pool_key.as_ref(),
        &[*ctx.bumps.get("buyside_sol_escrow_account").unwrap()],
    ]];

    check_allowlists_for_mint(
        &pool.allowlists,
        asset_mint,
        payer_asset_metadata,
        asset_master_edition,
    )?;

    let (total_price, next_price) =
        get_sol_total_price_and_next_price(pool, args.asset_amount, true)?;
    let lp_fee = get_sol_lp_fee(pool, buyside_sol_escrow_account.lamports(), total_price)?;
    let referral_fee = get_sol_referral_fee(pool, total_price)?;

    let transfer_asset_to = if pool.reinvest_fulfill_buy {
        sellside_escrow_token_account.to_account_info()
    } else {
        owner_token_account.to_account_info()
    };

    anchor_spl::token::transfer(
        CpiContext::new(
            token_program.to_account_info(),
            anchor_spl::token::Transfer {
                from: payer_asset_account.to_account_info(),
                to: transfer_asset_to,
                authority: payer.to_account_info(),
            },
        ),
        args.asset_amount,
    )?;

    // we can close the payer_asset_account if no amount left
    if payer_asset_account.amount == args.asset_amount {
        anchor_spl::token::close_account(CpiContext::new(
            token_program.to_account_info(),
            anchor_spl::token::CloseAccount {
                account: payer_asset_account.to_account_info(),
                destination: payer.to_account_info(),
                authority: payer.to_account_info(),
            },
        ))?;
    }

    // we can also close the pool escrow token account if we don't reinvest and its balance is 0
    if !pool.reinvest_fulfill_buy && sellside_escrow_token_account.amount == 0 {
        anchor_spl::token::close_account(CpiContext::new_with_signer(
            token_program.to_account_info(),
            anchor_spl::token::CloseAccount {
                account: sellside_escrow_token_account.to_account_info(),
                destination: payer.to_account_info(),
                authority: pool.to_account_info(),
            },
            pool_seeds,
        ))?;
    }

    // prevent frontrun by pool config changes
    let payment_amount = total_price
        .checked_sub(lp_fee)
        .ok_or(MMMErrorCode::NumericOverflow)?
        .checked_sub(referral_fee)
        .ok_or(MMMErrorCode::NumericOverflow)?;
    if payment_amount < args.min_payment_amount {
        return Err(MMMErrorCode::InvalidRequestedPrice.into());
    }

    anchor_lang::solana_program::program::invoke_signed(
        &anchor_lang::solana_program::system_instruction::transfer(
            buyside_sol_escrow_account.key,
            payer.key,
            payment_amount,
        ),
        &[
            buyside_sol_escrow_account.to_account_info(),
            payer.to_account_info(),
            system_program.to_account_info(),
        ],
        buyside_sol_escrow_account_seeds,
    )?;

    if lp_fee > 0 {
        anchor_lang::solana_program::program::invoke_signed(
            &anchor_lang::solana_program::system_instruction::transfer(
                buyside_sol_escrow_account.key,
                owner.key,
                lp_fee,
            ),
            &[
                buyside_sol_escrow_account.to_account_info(),
                owner.to_account_info(),
                system_program.to_account_info(),
            ],
            buyside_sol_escrow_account_seeds,
        )?;
    }

    if referral_fee > 0 {
        anchor_lang::solana_program::program::invoke_signed(
            &anchor_lang::solana_program::system_instruction::transfer(
                buyside_sol_escrow_account.key,
                referral.key,
                referral_fee,
            ),
            &[
                buyside_sol_escrow_account.to_account_info(),
                referral.to_account_info(),
                system_program.to_account_info(),
            ],
            buyside_sol_escrow_account_seeds,
        )?;
    }

    pool.sellside_orders_count = pool
        .sellside_orders_count
        .checked_add(args.asset_amount)
        .ok_or(MMMErrorCode::NumericOverflow)?;
    pool.lp_fee_earned = pool
        .lp_fee_earned
        .checked_add(lp_fee)
        .ok_or(MMMErrorCode::NumericOverflow)?;
    pool.spot_price = next_price;

    let royalty_paid = pay_creator_fees_in_sol(
        pool.buyside_creator_royalty_bp,
        total_price,
        payer_asset_metadata.to_account_info(),
        ctx.remaining_accounts,
        buyside_sol_escrow_account.to_account_info(),
        buyside_sol_escrow_account_seeds,
        system_program.to_account_info(),
    )?;

    try_close_pool(
        pool,
        *ctx.bumps.get("pool").unwrap(),
        owner.to_account_info(),
        system_program.to_account_info(),
        buyside_sol_escrow_account.lamports(),
    )?;

    msg!(
        "{{\"royalty_paid\":{},\"total_price\":{}}}",
        royalty_paid,
        total_price,
    );

    Ok(())
}
