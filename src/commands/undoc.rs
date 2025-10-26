use crate::service::iot::start_iot_client;
use std::sync::Arc;

#[derive(clap::Parser, Debug)]
pub struct UndocCommand {
    #[command(subcommand)]
    cmd: SubCommand,
}

#[derive(clap::Parser, Debug)]
enum SubCommand {
    DumpOneClick {},
    ShowOneClick {},
    OneClick { name: String },
}

impl UndocCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        match &self.cmd {
            SubCommand::DumpOneClick {} => {
                let client = args.undoc_args.api_client()?;
                let token = client.login_community().await?;
                let res = client.get_saved_one_click_shortcuts(&token).await?;

                println!("{res:#?}");
            }
            SubCommand::ShowOneClick {} => {
                let client = args.undoc_args.api_client()?;
                let items = client.parse_one_clicks().await?;
                println!("{items:#?}");
            }
            SubCommand::OneClick { name } => {
                let client = args.undoc_args.api_client()?;
                let items = client.parse_one_clicks().await?;
                let item = items
                    .iter()
                    .find(|item| &item.name == name)
                    .ok_or_else(|| anyhow::anyhow!("didn't find item {name}"))?;

                let state = Arc::new(crate::service::state::State::new());
                start_iot_client(&args.undoc_args, state.clone(), None).await?;
                let iot = state.get_iot_client().await.expect("just started iot");

                iot.activate_one_click(&item).await?;
            }
        }
        Ok(())
    }
}
