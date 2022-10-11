use anchor_lang::{prelude::*, AnchorDeserialize, AnchorSerialize};

use crate::state::Pool;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct WithdrawSellArgs{
    asset_amount: u64
}

#[derive(Accounts)]
#[instruction(args:WithdrawSellArgs)]
pub struct WithdrawSell<'info> {
    #[account(mut)]
    owner: Signer<'info>,
    /// CHECK: we will check cosigner when cosign field is on
    cosigner: UncheckedAccount<'info>,
    #[account(
        seeds = [b"mmm_pool", owner.key().as_ref(), pool.uuid.as_bytes()],
        bump
    )]
    pool: Account<'info, Pool>,
    system_program: Program<'info, System>
}
