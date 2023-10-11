use anyhow::{Context, Result};
use chrono::NaiveDateTime;
use clap::Parser;
use iota_sdk::client::{
    api::GetAddressesOptions,
    node_api::indexer::query_parameters::QueryParameter,
    secret::{private_key::PrivateKeySecretManager, SecretManager},
    Client,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use tabled::{
    settings::{Alignment, Style},
    Table, Tabled,
};

/// Simple program to display the timelocked balances of a list of private keys
#[derive(Parser, Debug)]
#[command(about, long_about = None)]
struct Args {
    /// Node URL to issue transactions with
    #[arg(short, long, env = "NODE_URL")]
    node_url: String,

    /// Currency to display the value in
    #[arg(short, long, default_value = "eur")]
    currency: String,

    /// Base58 encoded private keys
    #[arg(long, value_delimiter = ',', env = "PRIVATE_KEYS")]
    keys: Vec<String>,
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

    let mut balances = BTreeMap::new();
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
                QueryParameter::HasExpiration(false),
                QueryParameter::HasStorageDepositReturn(false),
            ])
            .await?;

        let outputs_responses = client.get_outputs(&output_ids_response.items).await?;

        for output in outputs_responses {
            let metadata = output.metadata();
            if metadata.is_spent() {
                continue;
            }

            let output = output.output();
            if output.amount() == 0 {
                continue;
            }

            // get timestamp of potential timelock
            let timelock = output
                .unlock_conditions()
                .and_then(|uc| uc.timelock().map(|tl| tl.timestamp()));
            // if there is no timelock, use the booking timestamp
            let ts = match timelock {
                Some(ts) => ts,
                None => metadata.milestone_timestamp_booked(),
            };

            // increment the balance for the timestamp
            *balances.entry(ts).or_insert(0) += output.amount();
        }
    }

    // get the price of IOTA
    let price = get_price(&args.currency).await?;
    // print the balances
    print_balances(balances, price, &args.currency)?;

    Ok(())
}

const PRICE_API_URL: &str = "https://api.coingecko.com/api/v3/simple/price";

async fn get_price(vs_currency: &str) -> Result<f64> {
    #[derive(Debug, Deserialize)]
    struct ApiResponse {
        iota: BTreeMap<String, f64>,
    }

    let client = reqwest::Client::new();
    let resp: ApiResponse = client
        .get(PRICE_API_URL)
        .query(&[
            ("ids", "iota"),
            ("vs_currencies", vs_currency),
            ("precision", "18"),
        ])
        .send()
        .await?
        .json()
        .await?;
    let price = *resp
        .iota
        .get(vs_currency)
        .with_context(|| format!("price in '{}' not found", vs_currency))?;

    Ok(price)
}

fn print_balances(balances: BTreeMap<u32, u64>, price: f64, currency: &str) -> Result<()> {
    #[derive(Tabled)]
    struct Row {
        unlock_time: NaiveDateTime,
        amount: String,
        value: String,
        cumulative_amount: String,
        cumulative_value: String,
    }

    let currency = currency.to_uppercase();

    let mut amounts = Vec::new();
    let mut cumulative = 0;
    for (ts, amount) in balances {
        cumulative += amount;
        let unlock_time =
            NaiveDateTime::from_timestamp_opt(ts.into(), 0).context("invalid timestamp")?;

        amounts.push(Row {
            unlock_time,
            amount: format!("{:.6} IOTA", amount as f64 / 1_000_000.),
            value: format!("{:.2} {}", amount as f64 / 1_000_000. * price, currency),
            cumulative_amount: format!("{:.6} IOTA", cumulative as f64 / 1_000_000.),
            cumulative_value: format!("{:.2} {}", cumulative as f64 / 1_000_000. * price, currency),
        });
    }

    let mut table = Table::new(amounts);
    table.with(Style::sharp()).with(Alignment::right());

    println!("{table}");

    Ok(())
}
