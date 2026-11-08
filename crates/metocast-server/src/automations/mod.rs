//! Automations: rules that run inside the core, so they work with no UI open, in the Mac app's
//! server mode and in the headless `metocast-server` alike. When one device does something,
//! Metocast does something on others.
//!
//! - `matching` holds the decisions, as pure functions.
//! - `arrival` waits for a camera to reach a preset; `run` runs a rule's actions in order.
//! - `store` keeps the rules; `api` is their REST surface.
//! - This module is the engine: it listens to the devices, fires rules, queues their runs and
//!   keeps the history. A single worker runs the queue, so a stop can't interleave with a start
//!   that is still in flight.

pub mod api;
pub mod arrival;
pub mod matching;
pub mod run;
pub mod store;

use metocast_core::best_effort::LogGone;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::Message;
use chrono::Utc;
use metocast_core::automations::{
    ActionOutcome, Automation, AutomationDraft, AutomationList, AutomationRun, AutomationTrigger,
    RecordTarget,
};
use serde_json::json;
use sqlx::PgPool;
use tokio::sync::broadcast::{self, error::RecvError};
use tokio::sync::{RwLock, mpsc, oneshot};
use uuid::Uuid;

use self::arrival::Camera;
use self::matching::{
    Cause, ChannelMemory, Mutes, chain_allows, channel_trigger_fires, cooling,
    preset_trigger_matches, still_holds,
};
use self::run::{Devices, Limits};
use self::store::StoreError;
use crate::connectors::preset_tracker::{Motion, PresetEvent, PresetFinished};
use crate::connectors::rodecaster::{MuteEvent, RodecasterConnector};
use crate::models::event::find_current_event;
use crate::recording_control::{self, Outcome};
use crate::server::AppState;

/// How many finished runs are kept, newest first.
pub const HISTORY: usize = 50;

type WsClients = Arc<RwLock<HashMap<Uuid, crate::server::ws_client::ClientTx>>>;

/// One run, waiting its turn.
pub struct Job {
    automation: Automation,
    cause: Cause,
    /// The rules that led here, this one last.
    chain: Vec<Uuid>,
    manual: bool,
    /// Where a caller waiting for the run gets it.
    reply: Option<oneshot::Sender<AutomationRun>>,
}

pub struct AutomationService {
    pool: PgPool,
    ws_clients: WsClients,
    rules: RwLock<Vec<Automation>>,
    armed: AtomicBool,
    last_fired: Mutex<HashMap<Uuid, Instant>>,
    history: Mutex<VecDeque<AutomationRun>>,
    queue: mpsc::UnboundedSender<Job>,
    /// The hold each trigger is waiting out, by the number it was started with. A channel that
    /// changes again drops the entry, and the timer finds it gone.
    holds: Mutex<HashMap<(Uuid, usize), u64>>,
    hold_serial: AtomicU64,
}

/// The devices of a running server.
struct AppDevices<'a> {
    state: &'a AppState,
}

impl Camera for AppDevices<'_> {
    fn motion(&self) -> Motion {
        self.state.middlecontrol_connector.current_motion()
    }

    fn preset_events(&self) -> broadcast::Receiver<PresetEvent> {
        self.state.middlecontrol_connector.preset_tx.subscribe()
    }

    async fn recall(
        &self,
        preset: u8,
        camera_id: Option<u8>,
        chain: Vec<Uuid>,
    ) -> Result<(), String> {
        self.state
            .middlecontrol_connector
            .recall_preset_for(preset, camera_id, chain)
            .await
    }
}

impl Devices for AppDevices<'_> {
    async fn ensure(&self, target: &RecordTarget, recording: bool) -> Result<Outcome, String> {
        recording_control::ensure(self.state, target, recording).await
    }
}

fn channel_mutes(profile: &rodecaster::Profile) -> Vec<(usize, Mutes)> {
    profile
        .channels
        .iter()
        .map(|channel| {
            (
                channel.channel,
                Mutes {
                    desk: channel.mute,
                    transmitter: channel.wireless_mute,
                },
            )
        })
        .collect()
}

/// Says what went wrong in a run, for the warning that goes to every client.
fn failure_message(run: &AutomationRun) -> String {
    const SHOWN: usize = 3;
    let failures: Vec<String> = run
        .results
        .iter()
        .filter(|result| result.outcome == ActionOutcome::Failed)
        .map(|result| match &result.message {
            Some(message) => format!("{} ({message})", result.action),
            None => result.action.clone(),
        })
        .collect();
    let more = failures.len().saturating_sub(SHOWN);
    let mut text = failures
        .into_iter()
        .take(SHOWN)
        .collect::<Vec<_>>()
        .join("; ");
    if more > 0 {
        text.push_str(&format!("; and {more} more"));
    }
    format!("Automation \"{}\" had problems: {text}", run.name)
}

impl AutomationService {
    #[must_use]
    pub fn new(pool: PgPool, ws_clients: WsClients) -> (Arc<Self>, mpsc::UnboundedReceiver<Job>) {
        let (queue, jobs) = mpsc::unbounded_channel();
        let service = Arc::new(Self {
            pool,
            ws_clients,
            rules: RwLock::new(Vec::new()),
            armed: AtomicBool::new(true),
            last_fired: Mutex::new(HashMap::new()),
            history: Mutex::new(VecDeque::new()),
            queue,
            holds: Mutex::new(HashMap::new()),
            hold_serial: AtomicU64::new(0),
        });
        (service, jobs)
    }

    /// Reads the rules and the master switch from the database.
    pub async fn load(&self) {
        match store::list(&self.pool).await {
            Ok(rules) => *self.rules.write().await = rules,
            Err(error) => tracing::error!("automations: cannot load the rules: {error}"),
        }
        self.armed
            .store(store::armed(&self.pool).await, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_armed(&self) -> bool {
        self.armed.load(Ordering::SeqCst)
    }

    pub async fn snapshot(&self) -> AutomationList {
        AutomationList {
            armed: self.is_armed(),
            automations: self.rules.read().await.clone(),
        }
    }

    /// The `automations.changed` message, which is also the answer to `automations.list`.
    pub async fn changed_message(&self) -> String {
        let list = self.snapshot().await;
        json!({
            "type": "automations.changed",
            "armed": list.armed,
            "automations": list.automations,
        })
        .to_string()
    }

    async fn broadcast(&self, message: String) {
        let clients = self.ws_clients.read().await;
        for client in clients.values() {
            client
                .send(Message::Text(message.clone().into()))
                .log_gone();
        }
    }

    /// After any write: refresh what the engine goes by and tell every client.
    async fn changed(&self) {
        self.load().await;
        self.broadcast(self.changed_message().await).await;
    }

    /// # Errors
    /// A rule that breaks a limit, or a database error.
    pub async fn create(&self, draft: &AutomationDraft) -> Result<Automation, StoreError> {
        let created = store::create(&self.pool, draft).await?;
        self.changed().await;
        Ok(created)
    }

    /// # Errors
    /// A rule that breaks a limit, no rule with that id, or a database error.
    pub async fn replace(
        &self,
        id: Uuid,
        draft: &AutomationDraft,
    ) -> Result<Automation, StoreError> {
        let replaced = store::replace(&self.pool, id, draft).await?;
        self.changed().await;
        Ok(replaced)
    }

    /// # Errors
    /// No rule with that id, or a database error.
    pub async fn delete(&self, id: Uuid) -> Result<(), StoreError> {
        store::delete(&self.pool, id).await?;
        self.last_fired.lock().expect("fire log lock").remove(&id);
        self.changed().await;
        Ok(())
    }

    /// # Errors
    /// A database error.
    pub async fn set_armed(&self, armed: bool) -> anyhow::Result<()> {
        store::set_armed(&self.pool, armed).await?;
        self.armed.store(armed, Ordering::SeqCst);
        self.changed().await;
        Ok(())
    }

    /// The last [`HISTORY`] runs, newest first.
    #[must_use]
    pub fn runs(&self) -> Vec<AutomationRun> {
        self.history
            .lock()
            .expect("history lock")
            .iter()
            .cloned()
            .collect()
    }

    async fn rule(&self, id: Uuid) -> Option<Automation> {
        self.rules
            .read()
            .await
            .iter()
            .find(|rule| rule.id == id)
            .cloned()
    }

    fn manual_job(rule: Automation, reply: Option<oneshot::Sender<AutomationRun>>) -> Job {
        Job {
            chain: vec![rule.id],
            automation: rule,
            cause: Cause::Manual,
            manual: true,
            reply,
        }
    }

    /// "Run now": queues the rule's actions, ignoring its cooldown, its event check and the
    /// master switch, and waits for the run. `None` when there is no such rule.
    pub async fn run_now(&self, id: Uuid) -> Option<AutomationRun> {
        let rule = self.rule(id).await?;
        let (reply, answer) = oneshot::channel();
        self.queue.send(Self::manual_job(rule, Some(reply))).ok()?;
        answer.await.ok()
    }

    /// "Run now" without waiting: the run arrives as `automation.run`. `false` when there is
    /// no such rule.
    pub async fn queue_now(&self, id: Uuid) -> bool {
        match self.rule(id).await {
            Some(rule) => self.queue.send(Self::manual_job(rule, None)).is_ok(),
            None => false,
        }
    }

    /// Whether a rule may fire now, and if so queues it. Everything that can say no does:
    /// the master switch, the rule's own switch, the chain it was fired through, the day's
    /// event and the cooldown.
    async fn fire(&self, rule: Automation, cause: Cause, chain: Vec<Uuid>) {
        if !self.is_armed() || !rule.enabled || !chain_allows(rule.id, &chain) {
            return;
        }
        let cooling_down =
            |last: Option<Instant>| cooling(last, Instant::now(), rule.cooldown_seconds);
        if cooling_down(
            self.last_fired
                .lock()
                .expect("fire log lock")
                .get(&rule.id)
                .copied(),
        ) {
            return;
        }
        if rule.require_event {
            match find_current_event(&self.pool).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    tracing::info!(automation = %rule.name, "not fired: no event today");
                    return;
                }
                Err(error) => {
                    tracing::warn!(automation = %rule.name, "not fired: {error}");
                    return;
                }
            }
        }
        {
            // Claimed under the lock, so two triggers at once start one run.
            let mut fired = self.last_fired.lock().expect("fire log lock");
            if cooling_down(fired.get(&rule.id).copied()) {
                return;
            }
            fired.insert(rule.id, Instant::now());
        }
        tracing::info!(automation = %rule.name, cause = %cause.describe(), "automation fired");
        let mut chain = chain;
        chain.push(rule.id);
        self.queue
            .send(Job {
                automation: rule,
                cause,
                chain,
                manual: false,
                reply: None,
            })
            .log_gone();
    }

    async fn on_preset(&self, finished: PresetFinished) {
        if !self.is_armed() {
            return;
        }
        let rules = self.rules.read().await.clone();
        for rule in rules.iter().filter(|rule| rule.enabled) {
            let fired = rule.triggers.iter().any(|trigger| {
                preset_trigger_matches(trigger, finished.preset, finished.camera_id)
            });
            if fired {
                let cause = Cause::Preset {
                    preset: finished.preset,
                    camera_id: finished.camera_id,
                };
                self.fire(rule.clone(), cause, finished.chain.clone()).await;
            }
        }
    }

    async fn on_mute(
        self: &Arc<Self>,
        desk: &Arc<RodecasterConnector>,
        memory: &mut ChannelMemory,
        event: MuteEvent,
    ) {
        // The memory follows every mute, armed or not, so the first edge after arming is right.
        let Some((before, after)) = memory.apply(event.channel, event.muted, event.remote) else {
            return;
        };
        if !self.is_armed() {
            return;
        }
        let rules = self.rules.read().await.clone();
        for rule in rules.iter().filter(|rule| rule.enabled) {
            for (index, trigger) in rule.triggers.iter().enumerate() {
                let AutomationTrigger::RodecasterChannel {
                    channel,
                    live,
                    source,
                    hold_seconds,
                    ..
                } = trigger
                else {
                    continue;
                };
                if usize::try_from(*channel) != Ok(event.channel) {
                    continue;
                }
                if before.live(*source) != after.live(*source) {
                    // The channel changed, so a wait on it starts over.
                    self.holds
                        .lock()
                        .expect("holds lock")
                        .remove(&(rule.id, index));
                }
                if channel_trigger_fires(trigger, event.channel, before, after) {
                    let cause = Cause::Channel {
                        label: event.label.clone(),
                        live: *live,
                    };
                    if *hold_seconds > 0.0 {
                        self.hold(desk, rule.id, index, cause, *hold_seconds);
                    } else {
                        self.fire(rule.clone(), cause, Vec::new()).await;
                    }
                    break;
                }
            }
        }
    }

    /// Waits out a trigger's hold, then fires if the channel is still as the trigger said. The
    /// channel is read from the profile Metocast keeps, which is updated before a mute is
    /// announced, so a cough-mute that was undone does not split the recording.
    fn hold(
        self: &Arc<Self>,
        desk: &Arc<RodecasterConnector>,
        rule: Uuid,
        index: usize,
        cause: Cause,
        seconds: f32,
    ) {
        let serial = self.hold_serial.fetch_add(1, Ordering::SeqCst) + 1;
        self.holds
            .lock()
            .expect("holds lock")
            .insert((rule, index), serial);
        let service = Arc::clone(self);
        let desk = Arc::clone(desk);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f32(seconds)).await;
            {
                let mut holds = service.holds.lock().expect("holds lock");
                if holds.get(&(rule, index)) != Some(&serial) {
                    return;
                }
                holds.remove(&(rule, index));
            }
            // The rule as it is now: it may have been edited, or deleted, in the meantime.
            let Some(current) = service.rule(rule).await else {
                return;
            };
            let Some(trigger) = current.triggers.get(index) else {
                return;
            };
            let AutomationTrigger::RodecasterChannel { channel, .. } = trigger else {
                return;
            };
            let mutes = desk.get_profile().and_then(|profile| {
                let channel = usize::try_from(*channel).ok()?;
                profile.channels.get(channel).map(|channel| Mutes {
                    desk: channel.mute,
                    transmitter: channel.wireless_mute,
                })
            });
            if mutes.is_some_and(|mutes| still_holds(trigger, mutes)) {
                service.fire(current.clone(), cause, Vec::new()).await;
            }
        });
    }

    /// Listens to the devices and fires rules. Nothing fires on server start, on a reconnect or
    /// resync, or when a rule is saved: only a change that happens while this runs.
    async fn listen(self: Arc<Self>, state: AppState) {
        let desk = &state.rodecaster_connector;
        let mut presets = state.middlecontrol_connector.preset_tx.subscribe();
        let mut mutes = desk.mute_tx.subscribe();
        let mut profiles = desk.profile_tx.subscribe();
        let mut memory = ChannelMemory::default();
        let reseed = |memory: &mut ChannelMemory| {
            if let Some(profile) = desk.get_profile() {
                memory.seed(channel_mutes(&profile));
            }
        };
        reseed(&mut memory);

        loop {
            tokio::select! {
                event = presets.recv() => match event {
                    Ok(PresetEvent::Finished(finished)) => self.on_preset(finished).await,
                    Ok(PresetEvent::Started(_)) => {}
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "automations: preset events were missed");
                    }
                    Err(RecvError::Closed) => return,
                },
                event = mutes.recv() => match event {
                    Ok(mute) => self.on_mute(&state.rodecaster_connector, &mut memory, mute).await,
                    Err(RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "automations: mute events were missed");
                        reseed(&mut memory);
                    }
                    Err(RecvError::Closed) => return,
                },
                event = profiles.recv() => match event {
                    Ok(profile) => memory.seed(channel_mutes(&profile)),
                    Err(RecvError::Lagged(_)) => reseed(&mut memory),
                    Err(RecvError::Closed) => return,
                },
            }
        }
    }

    /// Runs the queue, one run at a time.
    async fn work(self: Arc<Self>, state: AppState, mut jobs: mpsc::UnboundedReceiver<Job>) {
        while let Some(job) = jobs.recv().await {
            let started_at = Utc::now();
            let results = run::execute(
                &AppDevices { state: &state },
                &job.automation.actions,
                &job.cause,
                &job.chain,
                Limits::LIVE,
            )
            .await;
            let finished = AutomationRun {
                id: Uuid::new_v4(),
                automation_id: job.automation.id,
                name: job.automation.name.clone(),
                cause: job.cause.describe(),
                manual: job.manual,
                started_at,
                results,
            };
            self.finish(&finished).await;
            if let Some(reply) = job.reply {
                reply.send(finished).log_gone();
            }
        }
    }

    /// Logs a run, keeps it, tells every client, and warns them when something failed.
    async fn finish(&self, finished: &AutomationRun) {
        for result in &finished.results {
            tracing::info!(
                automation = %finished.name,
                action = %result.action,
                outcome = ?result.outcome,
                message = result.message.as_deref().unwrap_or(""),
                "automation action"
            );
        }
        {
            let mut history = self.history.lock().expect("history lock");
            history.push_front(finished.clone());
            history.truncate(HISTORY);
        }
        self.broadcast(json!({ "type": "automation.run", "run": finished }).to_string())
            .await;
        if finished.failed() {
            crate::server::websocket::broadcast_notification(
                &self.ws_clients,
                "warn",
                &failure_message(finished),
            )
            .await;
        }
    }
}

/// Loads the rules and starts the engine: the listener that fires rules and the worker that
/// runs them.
pub async fn spawn(state: AppState, jobs: mpsc::UnboundedReceiver<Job>) {
    let service = Arc::clone(&state.automations);
    service.load().await;
    tokio::spawn(Arc::clone(&service).work(state.clone(), jobs));
    tokio::spawn(service.listen(state));
}

#[cfg(test)]
mod tests {
    use metocast_core::automations::{ActionResult, AutomationAction, ChannelSource, RecordTarget};
    use rodecaster::{Channel, Profile};

    use super::*;

    fn run_with(results: Vec<ActionResult>) -> AutomationRun {
        AutomationRun {
            id: Uuid::nil(),
            automation_id: Uuid::nil(),
            name: "Service start".to_string(),
            cause: "Run now".to_string(),
            manual: true,
            started_at: Utc::now(),
            results,
        }
    }

    fn result(action: &str, outcome: ActionOutcome, message: Option<&str>) -> ActionResult {
        ActionResult {
            action: action.to_string(),
            outcome,
            message: message.map(str::to_string),
        }
    }

    #[test]
    fn a_warning_names_the_rule_and_what_failed() {
        let run = run_with(vec![
            result(
                "Start OBS",
                ActionOutcome::Failed,
                Some("obs_not_connected"),
            ),
            result("Start ATEM", ActionOutcome::Done, None),
            result("Start Metocast audio", ActionOutcome::Failed, None),
        ]);
        assert_eq!(
            failure_message(&run),
            "Automation \"Service start\" had problems: Start OBS (obs_not_connected); Start Metocast audio"
        );
    }

    #[test]
    fn a_warning_about_many_failures_stays_short() {
        let run = run_with(
            (1..=5)
                .map(|n| result(&format!("Start {n}"), ActionOutcome::Failed, Some("no")))
                .collect(),
        );
        assert_eq!(
            failure_message(&run),
            "Automation \"Service start\" had problems: Start 1 (no); Start 2 (no); Start 3 (no); and 2 more"
        );
    }

    // ── The engine without a database or a device ────────────────────────────────────────

    /// A service whose pool never connects: nothing here may reach the database.
    fn service() -> (
        Arc<AutomationService>,
        mpsc::UnboundedReceiver<Job>,
        mpsc::UnboundedReceiver<crate::server::ws_client::Outgoing>,
    ) {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .acquire_timeout(Duration::from_millis(200))
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .expect("a lazy pool");
        let clients: WsClients = Arc::new(RwLock::new(HashMap::new()));
        let (client, messages) = crate::server::ws_client::ClientTx::channel(false);
        clients
            .try_write()
            .expect("nobody holds the clients yet")
            .insert(Uuid::new_v4(), client);
        let (service, jobs) = AutomationService::new(pool, clients);
        (service, jobs, messages)
    }

    fn rule(triggers: Vec<AutomationTrigger>) -> Automation {
        Automation {
            id: Uuid::new_v4(),
            name: "Service start".to_string(),
            enabled: true,
            triggers,
            actions: vec![AutomationAction::RecordStart {
                target: RecordTarget::Obs,
            }],
            cooldown_seconds: 5.0,
            require_event: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn preset_rule(preset: u8) -> Automation {
        rule(vec![AutomationTrigger::MiddlecontrolPreset {
            preset,
            camera_id: None,
        }])
    }

    fn channel_rule(live: bool, hold_seconds: f32) -> Automation {
        rule(vec![AutomationTrigger::RodecasterChannel {
            channel: 1,
            label: "Pastor".to_string(),
            live,
            source: ChannelSource::Either,
            hold_seconds,
        }])
    }

    async fn install(service: &AutomationService, rules: Vec<Automation>) {
        *service.rules.write().await = rules;
    }

    fn finished(preset: u8, chain: Vec<Uuid>) -> PresetFinished {
        PresetFinished {
            preset,
            camera_id: 1,
            at: Utc::now(),
            chain,
        }
    }

    fn queued(jobs: &mut mpsc::UnboundedReceiver<Job>) -> Vec<Job> {
        std::iter::from_fn(|| jobs.try_recv().ok()).collect()
    }

    #[tokio::test]
    async fn a_finished_preset_queues_the_rules_that_wait_for_it() {
        let (service, mut jobs, _messages) = service();
        let (wanted, other) = (preset_rule(3), preset_rule(1));
        install(&service, vec![wanted.clone(), other]).await;

        service.on_preset(finished(3, Vec::new())).await;

        let queue = queued(&mut jobs);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].automation.id, wanted.id);
        assert!(!queue[0].manual);
        // The run's chain ends with the rule that fired it.
        assert_eq!(queue[0].chain, vec![wanted.id]);
        assert_eq!(
            queue[0].cause,
            Cause::Preset {
                preset: 3,
                camera_id: 1
            }
        );
    }

    #[tokio::test]
    async fn a_rule_fires_once_however_many_of_its_triggers_match() {
        let (service, mut jobs, _messages) = service();
        let twice = rule(vec![
            AutomationTrigger::MiddlecontrolPreset {
                preset: 3,
                camera_id: None,
            },
            AutomationTrigger::MiddlecontrolPreset {
                preset: 3,
                camera_id: Some(1),
            },
        ]);
        install(&service, vec![twice]).await;
        service.on_preset(finished(3, Vec::new())).await;
        assert_eq!(queued(&mut jobs).len(), 1);
    }

    #[tokio::test]
    async fn nothing_fires_while_disarmed_or_for_a_disabled_rule() {
        let (service, mut jobs, _messages) = service();
        let mut off = preset_rule(3);
        off.enabled = false;
        install(&service, vec![off, preset_rule(3)]).await;

        service.armed.store(false, Ordering::SeqCst);
        service.on_preset(finished(3, Vec::new())).await;
        assert!(queued(&mut jobs).is_empty());

        service.armed.store(true, Ordering::SeqCst);
        service.on_preset(finished(3, Vec::new())).await;
        // Only the enabled one.
        assert_eq!(queued(&mut jobs).len(), 1);
    }

    #[tokio::test]
    async fn a_chain_fires_the_second_rule_but_not_the_first_again() {
        let (service, mut jobs, _messages) = service();
        let (a, b) = (preset_rule(3), preset_rule(3));
        install(&service, vec![a.clone(), b.clone()]).await;

        // Rule A's recall made this move: A is in its chain, B is not.
        service.on_preset(finished(3, vec![a.id])).await;
        let queue = queued(&mut jobs);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].automation.id, b.id);
        assert_eq!(queue[0].chain, vec![a.id, b.id]);
    }

    #[tokio::test]
    async fn a_chain_of_three_rules_goes_no_further() {
        let (service, mut jobs, _messages) = service();
        install(&service, vec![preset_rule(3)]).await;
        service
            .on_preset(finished(
                3,
                vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()],
            ))
            .await;
        assert!(queued(&mut jobs).is_empty());
    }

    #[tokio::test]
    async fn a_refire_inside_the_cooldown_is_ignored_and_one_after_it_is_not() {
        let (service, mut jobs, _messages) = service();
        let mut quick = preset_rule(3);
        quick.cooldown_seconds = 0.2;
        install(&service, vec![quick]).await;

        service.on_preset(finished(3, Vec::new())).await;
        service.on_preset(finished(3, Vec::new())).await;
        assert_eq!(queued(&mut jobs).len(), 1);

        tokio::time::sleep(Duration::from_millis(250)).await;
        service.on_preset(finished(3, Vec::new())).await;
        assert_eq!(queued(&mut jobs).len(), 1);
    }

    #[tokio::test]
    async fn a_rule_that_needs_an_event_is_not_fired_when_the_events_cannot_be_read() {
        let (service, mut jobs, _messages) = service();
        let mut needs_event = preset_rule(3);
        needs_event.require_event = true;
        install(&service, vec![needs_event.clone()]).await;

        service.on_preset(finished(3, Vec::new())).await;
        assert!(queued(&mut jobs).is_empty());
        // And a failed look does not use up the cooldown.
        assert!(
            !service
                .last_fired
                .lock()
                .unwrap()
                .contains_key(&needs_event.id)
        );
    }

    #[tokio::test]
    async fn run_now_ignores_the_cooldown_and_the_master_switch() {
        let (service, mut jobs, _messages) = service();
        let manual = preset_rule(3);
        install(&service, vec![manual.clone()]).await;
        service.armed.store(false, Ordering::SeqCst);
        service
            .last_fired
            .lock()
            .unwrap()
            .insert(manual.id, Instant::now());

        assert!(service.queue_now(manual.id).await);
        assert!(!service.queue_now(Uuid::new_v4()).await);
        let queue = queued(&mut jobs);
        assert_eq!(queue.len(), 1);
        assert!(queue[0].manual);
        assert_eq!(queue[0].cause, Cause::Manual);
        assert_eq!(queue[0].chain, vec![manual.id]);
    }

    fn desk(pastor_muted: bool) -> Arc<RodecasterConnector> {
        let desk = Arc::new(RodecasterConnector::new());
        set_pastor(&desk, pastor_muted);
        desk
    }

    /// The profile the connector keeps: channel 1 is the pastor's, muted on the desk or not.
    fn set_pastor(desk: &RodecasterConnector, muted: bool) {
        let channel = |channel: usize, label: &str, mute: bool| Channel {
            channel,
            label: label.to_string(),
            source: None,
            level: 0.0,
            mute,
            wireless_mute: false,
            cue: false,
            processing: Vec::new(),
            settings: Vec::new(),
        };
        *desk.profile.write().unwrap() = Some(Profile {
            model: "RØDECaster Pro II".to_string(),
            firmware: "1.7.3".to_string(),
            channels: vec![channel(0, "Host", false), channel(1, "Pastor", muted)],
        });
    }

    fn mute_event(muted: bool) -> MuteEvent {
        MuteEvent {
            channel: 1,
            label: "Pastor".to_string(),
            muted,
            remote: false,
            notify: false,
        }
    }

    fn seeded(muted: bool) -> ChannelMemory {
        let mut memory = ChannelMemory::default();
        memory.seed([
            (0, Mutes::default()),
            (
                1,
                Mutes {
                    desk: muted,
                    transmitter: false,
                },
            ),
        ]);
        memory
    }

    #[tokio::test]
    async fn a_channel_going_live_fires_a_rule_with_no_hold_at_once() {
        let (service, mut jobs, _messages) = service();
        let goes_live = channel_rule(true, 0.0);
        install(&service, vec![goes_live.clone()]).await;
        let desk = desk(false);
        let mut memory = seeded(true);

        service.on_mute(&desk, &mut memory, mute_event(false)).await;

        let queue = queued(&mut jobs);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].automation.id, goes_live.id);
        assert_eq!(
            queue[0].cause,
            Cause::Channel {
                label: "Pastor".to_string(),
                live: true
            }
        );
    }

    #[tokio::test]
    async fn a_resync_and_a_change_the_other_way_fire_nothing() {
        let (service, mut jobs, _messages) = service();
        install(&service, vec![channel_rule(true, 0.0)]).await;
        let desk = desk(false);
        let mut memory = seeded(true);

        // Muting the pastor is the other edge, and the desk's resync is no edge at all.
        service.on_mute(&desk, &mut memory, mute_event(true)).await;
        memory.seed([(1, Mutes::default())]);
        assert!(queued(&mut jobs).is_empty());
    }

    #[tokio::test]
    async fn the_memory_follows_mutes_while_disarmed() {
        let (service, mut jobs, _messages) = service();
        install(&service, vec![channel_rule(true, 0.0)]).await;
        let desk = desk(false);
        let mut memory = seeded(true);
        service.armed.store(false, Ordering::SeqCst);

        service.on_mute(&desk, &mut memory, mute_event(false)).await;
        assert!(queued(&mut jobs).is_empty());
        assert_eq!(memory.get(1), Some(Mutes::default()));
    }

    #[tokio::test]
    async fn a_silence_that_lasts_the_whole_hold_stops_the_recording() {
        let (service, mut jobs, _messages) = service();
        let goes_silent = channel_rule(false, 0.15);
        install(&service, vec![goes_silent.clone()]).await;
        let desk = desk(true);
        let mut memory = seeded(false);

        service.on_mute(&desk, &mut memory, mute_event(true)).await;
        assert!(queued(&mut jobs).is_empty(), "not before the hold is up");

        tokio::time::sleep(Duration::from_millis(400)).await;
        let queue = queued(&mut jobs);
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].automation.id, goes_silent.id);
        assert_eq!(
            queue[0].cause,
            Cause::Channel {
                label: "Pastor".to_string(),
                live: false
            }
        );
    }

    #[tokio::test]
    async fn a_cough_mute_that_is_undone_inside_the_hold_stops_nothing() {
        let (service, mut jobs, _messages) = service();
        install(&service, vec![channel_rule(false, 0.3)]).await;
        let desk = desk(true);
        let mut memory = seeded(false);

        service.on_mute(&desk, &mut memory, mute_event(true)).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        set_pastor(&desk, false);
        service.on_mute(&desk, &mut memory, mute_event(false)).await;

        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(queued(&mut jobs).is_empty());
    }

    #[tokio::test]
    async fn a_hold_starts_over_when_the_channel_changes_and_changes_back() {
        let (service, mut jobs, _messages) = service();
        install(&service, vec![channel_rule(false, 0.4)]).await;
        let desk = desk(true);
        let mut memory = seeded(false);

        // Muted at 0, unmuted at 0.15, muted again at 0.3: the 0.4 s hold that matters is
        // the last one, which ends at 0.7.
        service.on_mute(&desk, &mut memory, mute_event(true)).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        set_pastor(&desk, false);
        service.on_mute(&desk, &mut memory, mute_event(false)).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        set_pastor(&desk, true);
        service.on_mute(&desk, &mut memory, mute_event(true)).await;

        // 0.5 s in: the first hold's deadline (0.4) has passed, and it must not have fired.
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(queued(&mut jobs).is_empty(), "the first hold was cancelled");

        tokio::time::sleep(Duration::from_millis(600)).await;
        assert_eq!(queued(&mut jobs).len(), 1);
    }

    #[tokio::test]
    async fn a_rule_deleted_during_its_hold_does_not_fire() {
        let (service, mut jobs, _messages) = service();
        install(&service, vec![channel_rule(false, 0.15)]).await;
        let desk = desk(true);
        let mut memory = seeded(false);

        service.on_mute(&desk, &mut memory, mute_event(true)).await;
        install(&service, Vec::new()).await;

        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(queued(&mut jobs).is_empty());
    }

    #[tokio::test]
    async fn a_finished_run_is_kept_broadcast_and_warned_about_when_it_failed() {
        let (service, _jobs, mut messages) = service();
        let failed = run_with(vec![result(
            "Start OBS",
            ActionOutcome::Failed,
            Some("obs_not_connected"),
        )]);

        service.finish(&failed).await;

        let run: serde_json::Value =
            serde_json::from_str(&text(messages.try_recv().expect("a run message"))).unwrap();
        assert_eq!(run["type"], "automation.run");
        assert_eq!(run["run"]["name"], "Service start");
        assert_eq!(run["run"]["results"][0]["outcome"], "failed");
        let warning: serde_json::Value =
            serde_json::from_str(&text(messages.try_recv().expect("a notification"))).unwrap();
        assert_eq!(warning["type"], "notification");
        assert_eq!(warning["level"], "warn");
        assert!(
            warning["message"]
                .as_str()
                .unwrap()
                .contains("obs_not_connected")
        );
        assert_eq!(service.runs().len(), 1);
    }

    #[tokio::test]
    async fn a_run_that_went_well_sends_no_warning() {
        let (service, _jobs, mut messages) = service();
        service
            .finish(&run_with(vec![result(
                "Start OBS",
                ActionOutcome::Done,
                None,
            )]))
            .await;
        assert!(messages.try_recv().is_ok());
        assert!(messages.try_recv().is_err(), "no notification");
    }

    #[tokio::test]
    async fn the_history_keeps_the_last_fifty_runs_newest_first() {
        let (service, _jobs, _messages) = service();
        for n in 0..(HISTORY + 5) {
            let mut finished = run_with(Vec::new());
            finished.name = format!("run {n}");
            service.finish(&finished).await;
        }
        let runs = service.runs();
        assert_eq!(runs.len(), HISTORY);
        assert_eq!(runs[0].name, format!("run {}", HISTORY + 4));
        assert_eq!(runs[HISTORY - 1].name, "run 5");
    }

    fn text(outgoing: crate::server::ws_client::Outgoing) -> String {
        match outgoing.message {
            Message::Text(text) => text.as_str().to_string(),
            other => panic!("expected text, got {other:?}"),
        }
    }
}
