use anchor_lang::{prelude::*, AnchorDeserialize, AnchorSerialize};

use crate::state::Pool;

#[derive(AnchorSerialize, AnchorDeserialize)]
pub struct ChangeSpotPriceArgs{
    price: u64
}

#[derive(Accounts)]
#[instruction(args:ChangeSpotPriceArgs)]
pub struct ChangeSpotPrice<'info> {
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
