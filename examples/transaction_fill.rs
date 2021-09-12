use std::str::FromStr;


use web3::{
    ethabi::ethereum_types::U256,
    types::{Address, TransactionParameters},
};

/// Below generates and signs a transaction offline, before transmitting it to a public node (eg Infura)
/// For sending a transaction to a local node that stores private keys (eg Ganache) see transaction_private
#[tokio::main]
async fn main() -> web3::Result {
    // Sign up at infura > choose the desired network (eg Rinkeby) > copy the endpoint url into the below
    // If you need test ether use a faucet, eg https://faucet.rinkeby.io/
    let transport = web3::transports::Http::new("http://localhost:7545")?;
    let web3 = web3::Web3::new(transport);

    // Insert the 20-byte "to" address in hex format (prefix with 0x)
    let to = Address::from_str("0xD4C5Bb4A404Eb80E15bC1A280B9333202ce022Ad").unwrap();

    // Insert the 32-byte private key in hex format (do NOT prefix with 0x)
    let from = Address::from_str("0x38dbD7FDE7A732e1DE62f19e7d83529bdEb3F814").unwrap();

    // Build the tx object
    let tx_object = TransactionParameters {
        to: Some(to),
        value: U256::exp10(17), //0.1 eth
        ..Default::default()
    };

    let tx_filled = web3.accounts().fill_transaction(tx_object,&from).await?;

    println!("{:?}",tx_filled.tx.tx_hash(tx_filled.chain_id));
    Ok(())
}
