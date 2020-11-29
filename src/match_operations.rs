use crate::{
    endpoints::PlayerMatchRegistrationRequest,
    stores::match_store::Match,
    stores::{
        court_store::CourtStore,
        match_store::MatchStore,
        player_registration_store::{PlayerMatchRegistration, PlayerRegistrationStore},
        player_store::Player,
        player_store::PlayerStore,
    },
    ServerError,
};
use chrono::{Local, NaiveDate, NaiveDateTime};
use futures::future;
use serde::Serialize;
use tracing::warn;
#[derive(Debug, Serialize, PartialEq)]
pub struct MatchInfo {
    id: i64,
    class: String,
    player_one: Player,
    player_two: Player,
    player_one_arrived: bool,
    player_two_arrived: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    court: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    winner: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<String>,
    start_time: NaiveDateTime,
}

impl MatchInfo {
    fn from_components(match_data: Match, player_one: Player, player_two: Player) -> Self {
        MatchInfo {
            id: match_data.id,
            class: match_data.class,
            start_time: match_data.start_time,
            player_one_arrived: false,
            player_two_arrived: false,
            player_one,
            player_two,
            winner: None,
            result: None,
            court: None,
        }
    }
}

pub async fn register_player_to_match<S>(
    storage: &S,
    match_id: i64,
    mut request: PlayerMatchRegistrationRequest,
) -> Result<PlayerMatchRegistration, ServerError>
where
    S: MatchStore + PlayerRegistrationStore + PlayerStore + CourtStore,
{
    let match_data = storage.get_match(match_id).await?;

    if match_data.is_none() {
        return Err(ServerError::MatchNotFound);
    }
    let match_data = match_data.unwrap();

    if match_data.player_one != request.player_id && match_data.player_two != request.player_id {
        return Err(ServerError::InvalidPlayerRegistration);
    }

    let previous_registrations = storage.get_registered_players(match_id).await?;

    if previous_registrations
        .iter()
        .any(|registration| registration.player_id == request.player_id)
    {
        return Err(ServerError::PlayerAlreadyReigstered);
    }

    let second_player_registerd = !previous_registrations.is_empty();

    let registered_by = std::mem::take(&mut request.registered_by);
    let match_registration = storage
        .insert_player_registration(request.player_id, match_id, registered_by)
        .await?;

    if second_player_registerd {
        start_match(match_id, storage).await?;
    }
    Ok(match_registration)
}

pub struct TournamentMatchList {
    scheduled: Vec<MatchInfo>,
    playing: Vec<MatchInfo>,
    finished: Vec<MatchInfo>,
}

#[tracing::instrument(name = "Get tournament match list", skip(storage))]
pub async fn get_tournament_matches<S: MatchStore + PlayerStore>(
    tournament_id: i32,
    date: NaiveDate,
    storage: &S,
) -> Result<TournamentMatchList, ServerError> {
    let query_result = storage.get_tournament_matches(tournament_id).await?;

    let mut finished = Vec::new();
    let mut playing = Vec::new();
    let scheduled = Vec::new();

    for match_data in query_result.iter() {
         match get_match_player_info(storage, match_data).await {
             Ok(player_match_info ) => {
                
             }
             Err(err) => {
                 warn!("Player info not found for match: {}", err);
             }
         }
    }
}

#[tracing::instrument(name = "Start match", skip(storage))]
pub async fn start_match<S>(match_id: i64, storage: &S) -> Result<MatchInfo, ServerError>
where
    S: MatchStore + PlayerRegistrationStore + PlayerStore + CourtStore,
{
    let match_data = storage.get_match(match_id).await?;

    if match_data.is_none() {
        return Err(ServerError::MatchNotFound);
    }
    let match_data = match_data.unwrap();

    // check if court alreay has assigned court
    if storage
        .get_match_court(match_data.tournament_id, match_data.id)
        .await?
        .is_some()
    {
        return Err(ServerError::MatchAlreadyStarted);
    }

    // in finish match
    // free up court
    // pop court queue

    // check player registration
    let registered_players = storage.get_registered_players(match_id).await?;
    if registered_players.len() != 2 {
        return Err(ServerError::PlayerMissing);
    }
    if let (Ok(Some(player_one)), Ok(Some(player_two))) = future::join(
        storage.get_player(match_data.player_one),
        storage.get_player(match_data.player_two),
    )
    .await
    {
        // if no court assigned and players are present
        // try to assign free court
        if let Some(assigned_court) = storage
            .try_assign_free_court(match_data.tournament_id, match_data.id)
            .await?
        {
            let match_info = MatchInfo {
                court: Some(assigned_court),
                player_one_arrived: true,
                player_two_arrived: true,
                start_time: Local::now().naive_local(),
                ..MatchInfo::from_components(match_data, player_one, player_two)
            };
            Ok(match_info)
        } else {
            let court =
                append_to_queue_and_get_placement(storage, match_data.tournament_id, match_id)
                    .await?;
            let match_info = MatchInfo {
                court: Some(court),
                player_one_arrived: true,
                player_two_arrived: true,
                ..MatchInfo::from_components(match_data, player_one, player_two)
            };
            Ok(match_info)
        }
    } else {
        Err(ServerError::PlayerNotFound)
    }
}

// HELPERS:
struct PlayerMatchInfo {
    first_player_name: String,
    first_player_arrived: bool,
    second_player_name: String,
    second_player_arrived: bool,
}

async fn get_match_player_info<S: PlayerStore + PlayerRegistrationStore>(
    storage: &S,
    match_data: &Match,
) -> Result<PlayerMatchInfo, ServerError> {
    if let (Ok(Some(first_player)), Ok(Some(second_player))) = future::join(
        storage.get_player(match_data.player_one),
        storage.get_player(match_data.player_two),
    )
    .await
    {
        let registered_players = storage.get_registered_players(match_data.id).await?;
        let first_player_arrived = registered_players
            .iter()
            .find_map(|reg| Some(reg.player_id == first_player.id))
            .ok_or(ServerError::PlayerMissing)?;
        let second_player_arrived = registered_players
            .iter()
            .find_map(|reg| Some(reg.player_id == second_player.id))
            .ok_or(ServerError::PlayerMissing)?;

        Ok(PlayerMatchInfo {
            first_player_arrived,
            second_player_arrived,
            first_player_name: first_player.name,
            second_player_name: second_player.name,
        })
    } else {
        Err(ServerError::PlayerNotFound)
    }
}

async fn append_to_queue_and_get_placement(
    storage: &impl CourtStore,
    tournament_id: i32,
    match_id: i64,
) -> Result<String, sqlx::Error> {
    storage.append_court_queue(tournament_id, match_id).await?;
    let placement = storage
        .get_court_queue_placement(tournament_id, match_id)
        .await?;
    Ok(match placement {
        1 => "Först i kön",
        2 => "Andra plats i kön",
        _ => "Köplats: {}",
    }
    .into())
}