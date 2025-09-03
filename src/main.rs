use warp::Filter;
use serde_json::Value;
use reqwest::Client;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use tokio::sync::RwLock;
use std::time::{Duration, SystemTime};

const RPC_URL: &str = "https://api.mainnet-beta.solana.com";
const TOKEN_LIST_URL: &str = "https://raw.githubusercontent.com/solana-labs/token-list/main/src/tokens/solana.tokenlist.json";

static CLIENT: Lazy<Client> = Lazy::new(|| Client::new());
static TOKEN_MAP: Lazy<RwLock<(HashMap<String, Value>, SystemTime)>> =
    Lazy::new(|| RwLock::new((HashMap::new(), SystemTime::now())));

async fn refresh_token_map() -> Result<(), reqwest::Error> {
    let token_list: Value = CLIENT.get(TOKEN_LIST_URL).send().await?.json().await?;
    let mut token_map = HashMap::new();
    if let Some(tokens) = token_list["tokens"].as_array() {
        for token in tokens {
            if let Some(mint) = token["address"].as_str() {
                token_map.insert(mint.to_string(), token.clone());
            }
        }
    }
    let mut cache = TOKEN_MAP.write().await;
    *cache = (token_map, SystemTime::now());
    Ok(())
}

async fn get_token_map() -> Result<HashMap<String, Value>, reqwest::Error> {
    let cache = TOKEN_MAP.read().await;
    if cache.1.elapsed().unwrap_or(Duration::from_secs(0)) > Duration::from_secs(3600) {
        drop(cache);
        refresh_token_map().await?;
    }
    Ok(TOKEN_MAP.read().await.0.clone())
}

#[tokio::main(worker_threads = 8)]
async fn main() {
    let tokens_route = warp::path!("tokens" / String)
        .and_then(|wallet: String| async move {
            match get_spl_tokens(&wallet).await {
                Ok(tokens) => Ok::<_, warp::Rejection>(warp::reply::json(&tokens)),
                Err(_) => Ok::<_, warp::Rejection>(warp::reply::json(&serde_json::json!({"error": "Failed to fetch tokens"}))),
            }
        });

    let balance_route = warp::path!("balance" / String)
        .and_then(|wallet: String| async move {
            match get_sol_balance(&wallet).await {
                Ok(balance) => Ok::<_, warp::Rejection>(warp::reply::json(&balance)),
                Err(_) => Ok::<_, warp::Rejection>(warp::reply::json(&serde_json::json!({"error": "Failed to fetch balance"}))),
            }
        });

    let routes = tokens_route.or(balance_route);

    println!("Solana API running at http://127.0.0.1:3030");
    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn get_spl_tokens(wallet: &str) -> Result<Value, reqwest::Error> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenAccountsByOwner",
        "params": [
            wallet,
            { "programId": "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA" },
            { "encoding": "jsonParsed" }
        ]
    });

    let (rpc_resp, token_map) = tokio::join!(
        CLIENT.post(RPC_URL).json(&body).send(),
        get_token_map()
    );

    let resp: Value = rpc_resp?.json().await?;
    let token_accounts = resp["result"]["value"]
        .as_array()
        .cloned()
        .unwrap_or_else(Vec::new);


    let token_map = token_map?;

    let mut enriched_tokens = Vec::with_capacity(token_accounts.len());
    for account in token_accounts {
        if let Some(mint) = account["account"]["data"]["parsed"]["info"]["mint"].as_str() {
            let amount_str = account["account"]["data"]["parsed"]["info"]["tokenAmount"]["amount"].as_str().unwrap_or("0");
            let decimals = account["account"]["data"]["parsed"]["info"]["tokenAmount"]["decimals"].as_u64().unwrap_or(0);
            let amount = amount_str.parse::<f64>().unwrap_or(0.0) / 10f64.powi(decimals as i32);

            let mut token_info = serde_json::json!({
                "mint": mint,
                "amount": amount,
                "decimals": decimals
            });

            if let Some(metadata) = token_map.get(mint) {
                if let Some(symbol) = metadata["symbol"].as_str() { token_info["symbol"] = Value::String(symbol.to_string()); }
                if let Some(name) = metadata["name"].as_str() { token_info["name"] = Value::String(name.to_string()); }
                if let Some(logo_uri) = metadata["logoURI"].as_str() { token_info["logoURI"] = Value::String(logo_uri.to_string()); }
            }

            enriched_tokens.push(token_info);
        }
    }

    Ok(serde_json::json!(enriched_tokens))
}

async fn get_sol_balance(wallet: &str) -> Result<Value, reqwest::Error> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [wallet]
    });

    let resp: Value = CLIENT.post(RPC_URL).json(&body).send().await?.json().await?;
    let lamports = resp["result"]["value"].as_u64().unwrap_or(0);

    Ok(serde_json::json!({
        "lamports": lamports,
        "sol": lamports as f64 / 1_000_000_000.0
    }))
}