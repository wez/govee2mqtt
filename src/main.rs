mod lan_api;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();
    println!("woot");
    let (client, mut scan) = lan_api::Client::new().await?;

    while let Some(device) = scan.recv().await {
        println!("{device:?}");

        if let Ok(resp) = client.query_status(&device).await {
            println!("Got status: {resp:?}");
        }
    }

    Ok(())
}
