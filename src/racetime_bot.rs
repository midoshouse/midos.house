use {
    async_trait::async_trait,
    racetime::{
        Error,
        handler::{
            RaceContext,
            RaceHandler,
        },
        model::*,
    },
    crate::config::ConfigRaceTime,
};

struct Handler;

#[async_trait]
impl RaceHandler for Handler {
    fn should_handle(race_data: &RaceData) -> Result<bool, Error> {
        Ok(
            race_data.goal.name == "3rd Multiworld Tournament" //TODO don't hardcode (use a list shared with RandoBot?)
            && race_data.goal.custom
            && !matches!(race_data.status.value, RaceStatusValue::Finished | RaceStatusValue::Cancelled)
        )
    }

    async fn new(ctx: &RaceContext) -> Result<Self, Error> {
        //TODO different behavior for race rooms opened by the bot itself
        ctx.send_message("Hi! This goal name is reserved for an event. To get access to RandoBot commands, change the goal to something else.").await?; //TODO offer to roll a seed with tournament settings (or with draft changes)
        ctx.send_message("You can learn more about the event at https://midos.house/event/mw/3").await?;
        Ok(Self)
    }
}

pub(crate) async fn main(config: ConfigRaceTime, shutdown: rocket::Shutdown) -> Result<(), Error> {
    let bot = racetime::Bot::new("ootr", &config.client_id, &config.client_secret).await?;
    let () = bot.run_until::<Handler, _, _>(shutdown).await?;
    Ok(())
}
