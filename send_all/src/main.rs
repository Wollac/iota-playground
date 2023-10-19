use anyhow::Result;
use clap::Parser;
use iota_sdk::{
    client::{
        api::GetAddressesOptions,
        node_api::indexer::query_parameters::QueryParameter,
        secret::{private_key::PrivateKeySecretManager, SecretManager},
        Client,
    },
    types::block::address::Bech32Address,
    types::block::output::{unlock_condition::AddressUnlockCondition, BasicOutputBuilder},
};

/// Simple program to send all unlocked fonds of a list of private keys to a designated address.
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Node URL to issue transactions with
    #[arg(short, long, env = "NODE_URL")]
    node_url: String,

    /// Base58 encoded private keys
    #[arg(long, value_delimiter = ',', env = "PRIVATE_KEYS")]
    keys: Vec<String>,

    /// Recipient address
    #[arg(long, env = "RECIPIENT_ADDRESS")]
    recipient_address: Bech32Address,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv()?;
    let args = Args::parse();

    // Create a node client
    let client = Client::builder()
        .with_node(&args.node_url)?
        .finish()
        .await?;

    let token_supply = client.get_token_supply().await?;
    let now = client.get_time_checked().await?;

    for base58 in args.keys {
        let secret_manager = SecretManager::from(PrivateKeySecretManager::try_from_b58(base58)?);

        // Generate the first address
        let mut addresses = secret_manager
            .generate_ed25519_addresses(
                GetAddressesOptions::from_client(&client)
                    .await?
                    .with_account_index(0)
                    .with_range(0..1),
            )
            .await?;
        let address = addresses.pop().unwrap();

        // Get output ids of outputs that can be controlled by this address without further unlock constraints
        let output_ids_response = client
            .basic_output_ids([
                QueryParameter::Address(address),
                QueryParameter::HasStorageDepositReturn(false),
            ])
            .await?;

        let outputs_responses = client.get_outputs(&output_ids_response.items).await?;

        let mut total_amount = 0;
        for output in outputs_responses {
            let metadata = output.metadata();
            if metadata.is_spent() {
                continue;
            }

            let output = output.output();

            let locked = output
                .unlock_conditions()
                .map_or(false, |uc| uc.is_time_locked(now));
            let expired = output
                .unlock_conditions()
                .map_or(false, |uc| uc.is_expired(now));

            if !locked && !expired {
                total_amount += output.amount();
            }
        }
        if total_amount == 0 {
            println!("No funds to send from {}", address);
            continue;
        }

        println!(
            "Sending {:.6} IOTA from {} to {}",
            total_amount as f64 / 1_000_000.0,
            address,
            args.recipient_address
        );

        let basic_output_builder = BasicOutputBuilder::new_with_amount(total_amount)
            .add_unlock_condition(AddressUnlockCondition::new(args.recipient_address));
        let output = basic_output_builder.finish_output(token_supply)?;

        let block = client
            .build_block()
            .with_secret_manager(&secret_manager)
            .with_outputs([output])?
            .finish()
            .await?;
        println!("Block with all outputs sent: {}", block.id());

        let _ = client.retry_until_included(&block.id(), None, None).await?;
        println!("Block with all outputs included: {}", block.id());
    }

    Ok(())
}
