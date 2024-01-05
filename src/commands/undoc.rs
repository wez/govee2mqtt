#[derive(clap::Parser, Debug)]
pub struct UndocCommand {
    #[command(subcommand)]
    cmd: SubCommand,
}

#[derive(clap::Parser, Debug)]
enum SubCommand {
    ShowOneClick {},
}

impl UndocCommand {
    pub async fn run(&self, args: &crate::Args) -> anyhow::Result<()> {
        match &self.cmd {
            SubCommand::ShowOneClick {} => {
                let client = args.undoc_args.api_client()?;
                let token = client.login_community().await?;
                let res = client.get_saved_one_click_shortcuts(&token).await?;
                println!("{res:#?}");

                println!("-------------------");

                for group in res {
                    for oc in group.one_clicks {
                        if oc.iot_rules.is_empty() {
                            continue;
                        }

                        let name = format!("{}: {}", group.name, oc.name);
                        println!("{name}");
                        for rule in oc.iot_rules {
                            println!("    {} ({})", rule.device_obj.name, rule.device_obj.device);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
