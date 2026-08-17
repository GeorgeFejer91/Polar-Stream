use std::{
    fmt,
    future::Future,
    time::{Duration, Instant},
};

use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SessionStage {
    AddressToDevice,
    GattSessionCreate,
    MaintainConnection,
    ConnectionSettle,
    PmdServiceDiscovery,
    PmdControlAccess,
    PmdControlDiscoveryUncached,
    PmdControlDiscoveryCached,
    PmdDataAccess,
    PmdDataDiscoveryUncached,
    PmdDataDiscoveryCached,
    HeartRateServiceDiscovery,
    HeartRateAccess,
    HeartRateDiscoveryUncached,
    HeartRateDiscoveryCached,
    BatteryServiceDiscovery,
    BatteryAccess,
    BatteryDiscoveryUncached,
    BatteryDiscoveryCached,
    HeartRateNotification,
    PmdControlNotification,
    PmdDataNotification,
    RequestEcgSettings,
    EcgSettingsResponse,
    EcgSettingsSettle,
    StartEcg,
    StartEcgResponse,
    StartAcc,
    StartAccResponse,
    FirstEcgFrame,
    FirstAccFrame,
    BatteryRead,
}

impl SessionStage {
    #[cfg(test)]
    const ALL: [Self; 32] = [
        Self::AddressToDevice,
        Self::GattSessionCreate,
        Self::MaintainConnection,
        Self::ConnectionSettle,
        Self::PmdServiceDiscovery,
        Self::PmdControlAccess,
        Self::PmdControlDiscoveryUncached,
        Self::PmdControlDiscoveryCached,
        Self::PmdDataAccess,
        Self::PmdDataDiscoveryUncached,
        Self::PmdDataDiscoveryCached,
        Self::HeartRateServiceDiscovery,
        Self::HeartRateAccess,
        Self::HeartRateDiscoveryUncached,
        Self::HeartRateDiscoveryCached,
        Self::BatteryServiceDiscovery,
        Self::BatteryAccess,
        Self::BatteryDiscoveryUncached,
        Self::BatteryDiscoveryCached,
        Self::HeartRateNotification,
        Self::PmdControlNotification,
        Self::PmdDataNotification,
        Self::RequestEcgSettings,
        Self::EcgSettingsResponse,
        Self::EcgSettingsSettle,
        Self::StartEcg,
        Self::StartEcgResponse,
        Self::StartAcc,
        Self::StartAccResponse,
        Self::FirstEcgFrame,
        Self::FirstAccFrame,
        Self::BatteryRead,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::AddressToDevice => "address-to-device",
            Self::GattSessionCreate => "gatt-session-create",
            Self::MaintainConnection => "maintain-connection",
            Self::ConnectionSettle => "connection-settle",
            Self::PmdServiceDiscovery => "pmd-service-discovery-uncached",
            Self::PmdControlAccess => "pmd-control-service-access",
            Self::PmdControlDiscoveryUncached => "pmd-control-discovery-uncached",
            Self::PmdControlDiscoveryCached => "pmd-control-discovery-cached",
            Self::PmdDataAccess => "pmd-data-service-access",
            Self::PmdDataDiscoveryUncached => "pmd-data-discovery-uncached",
            Self::PmdDataDiscoveryCached => "pmd-data-discovery-cached",
            Self::HeartRateServiceDiscovery => "heart-rate-service-discovery-uncached",
            Self::HeartRateAccess => "heart-rate-service-access",
            Self::HeartRateDiscoveryUncached => "heart-rate-discovery-uncached",
            Self::HeartRateDiscoveryCached => "heart-rate-discovery-cached",
            Self::BatteryServiceDiscovery => "battery-service-discovery-uncached",
            Self::BatteryAccess => "battery-service-access",
            Self::BatteryDiscoveryUncached => "battery-discovery-uncached",
            Self::BatteryDiscoveryCached => "battery-discovery-cached",
            Self::HeartRateNotification => "heart-rate-notification-subscription",
            Self::PmdControlNotification => "pmd-control-notification-subscription",
            Self::PmdDataNotification => "pmd-data-notification-subscription",
            Self::RequestEcgSettings => "pmd-request-ecg-settings",
            Self::EcgSettingsResponse => "pmd-ecg-settings-response",
            Self::EcgSettingsSettle => "pmd-ecg-settings-settle",
            Self::StartEcg => "pmd-start-ecg",
            Self::StartEcgResponse => "pmd-start-ecg-response",
            Self::StartAcc => "pmd-start-acc",
            Self::StartAccResponse => "pmd-start-acc-response",
            Self::FirstEcgFrame => "first-ecg-frame",
            Self::FirstAccFrame => "first-acc-frame",
            Self::BatteryRead => "battery-read",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StageResultClass {
    Success,
    SuccessCleanupError,
    NativeError,
    NativeErrorCleanupError,
    Timeout,
    TimeoutCancelled,
    TimeoutCleanupError,
    Cancelled,
    CancelledCleanupError,
}

impl StageResultClass {
    const fn name(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::SuccessCleanupError => "success-cleanup-error",
            Self::NativeError => "native-error",
            Self::NativeErrorCleanupError => "native-error-cleanup-error",
            Self::Timeout => "timeout",
            Self::TimeoutCancelled => "timeout-cancelled",
            Self::TimeoutCleanupError => "timeout-cleanup-error",
            Self::Cancelled => "cancelled",
            Self::CancelledCleanupError => "cancelled-cleanup-error",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StageFailure {
    stage: SessionStage,
    class: StageResultClass,
    message: String,
}

impl StageFailure {
    fn new(stage: SessionStage, class: StageResultClass, message: String) -> Self {
        Self {
            stage,
            class,
            message,
        }
    }

    #[cfg(test)]
    fn class(&self) -> StageResultClass {
        self.class
    }
}

impl fmt::Display for StageFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct StageReporter {
    enabled: bool,
    deadline: Option<tokio::time::Instant>,
}

impl StageReporter {
    pub(crate) const fn new(enabled: bool, deadline: Option<tokio::time::Instant>) -> Self {
        Self { enabled, deadline }
    }

    pub(crate) fn limit(self, requested: Duration) -> Duration {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
            .map_or(requested, |remaining| requested.min(remaining))
    }

    pub(crate) fn enter(self, stage: SessionStage, attempt: usize) -> StageSpan {
        if self.enabled {
            eprintln!(
                "POLAR_H10_SESSION_STAGE stage={} attempt={} transition=enter",
                stage.name(),
                attempt
            );
        }
        StageSpan {
            reporter: self,
            stage,
            attempt,
            started: Instant::now(),
            completed: false,
        }
    }

    pub(crate) fn record_immediate(
        self,
        stage: SessionStage,
        attempt: usize,
        class: StageResultClass,
    ) {
        self.enter(stage, attempt).finish(class);
    }
}

pub(crate) struct StageSpan {
    reporter: StageReporter,
    stage: SessionStage,
    attempt: usize,
    started: Instant,
    completed: bool,
}

impl StageSpan {
    pub(crate) fn finish(mut self, class: StageResultClass) {
        self.completed = true;
        if self.reporter.enabled {
            eprintln!(
                "POLAR_H10_SESSION_STAGE stage={} attempt={} transition=exit duration_ms={} result={}",
                self.stage.name(),
                self.attempt,
                self.started.elapsed().as_millis(),
                class.name()
            );
        }
    }
}

impl Drop for StageSpan {
    fn drop(&mut self) {
        if !self.completed && self.reporter.enabled {
            eprintln!(
                "POLAR_H10_SESSION_STAGE stage={} attempt={} transition=exit duration_ms={} result=owner-dropped",
                self.stage.name(),
                self.attempt,
                self.started.elapsed().as_millis()
            );
        }
    }
}

pub(crate) trait StageControl {
    fn cancel(&self) -> Result<(), String>;
    fn close(&self) -> Result<(), String>;

    fn close_after_success(&self) -> bool {
        true
    }
}

pub(crate) async fn run_controlled_stage<T, F, C>(
    reporter: StageReporter,
    stage: SessionStage,
    attempt: usize,
    duration: Duration,
    cancelled: &mut watch::Receiver<bool>,
    future: F,
    control: C,
) -> Result<T, StageFailure>
where
    F: Future<Output = Result<T, String>>,
    C: StageControl,
{
    let span = reporter.enter(stage, attempt);
    if *cancelled.borrow() {
        return Err(cancel_operation(
            span,
            stage,
            StageResultClass::Cancelled,
            StageResultClass::CancelledCleanupError,
            "setup was cancelled",
            &control,
        ));
    }

    tokio::pin!(future);
    let deadline = tokio::time::sleep(reporter.limit(duration));
    tokio::pin!(deadline);

    tokio::select! {
        result = &mut future => {
            match result {
                Ok(value) => {
                    let close_result = if control.close_after_success() {
                        control.close()
                    } else {
                        Ok(())
                    };
                    span.finish(if close_result.is_ok() {
                        StageResultClass::Success
                    } else {
                        StageResultClass::SuccessCleanupError
                    });
                    Ok(value)
                }
                Err(error) => {
                    let close_result = control.close();
                    let class = if close_result.is_ok() {
                        StageResultClass::NativeError
                    } else {
                        StageResultClass::NativeErrorCleanupError
                    };
                    span.finish(class);
                    Err(StageFailure::new(
                        stage,
                        class,
                        format!("Windows WinRT {} failed: {error}", stage.name()),
                    ))
                }
            }
        }
        changed = cancelled.changed() => {
            let reason = if changed.is_err() {
                "setup cancellation owner closed"
            } else {
                "setup was cancelled"
            };
            Err(cancel_operation(
                span,
                stage,
                StageResultClass::Cancelled,
                StageResultClass::CancelledCleanupError,
                reason,
                &control,
            ))
        }
        () = &mut deadline => {
            Err(cancel_operation(
                span,
                stage,
                StageResultClass::TimeoutCancelled,
                StageResultClass::TimeoutCleanupError,
                "timed out",
                &control,
            ))
        }
    }
}

fn cancel_operation<C: StageControl>(
    span: StageSpan,
    stage: SessionStage,
    clean_class: StageResultClass,
    cleanup_error_class: StageResultClass,
    reason: &str,
    control: &C,
) -> StageFailure {
    let cancel_result = control.cancel();
    let close_result = control.close();
    let class = if cancel_result.is_ok() && close_result.is_ok() {
        clean_class
    } else {
        cleanup_error_class
    };
    span.finish(class);
    StageFailure::new(
        stage,
        class,
        format!("Windows WinRT {} {reason}", stage.name()),
    )
}

pub(crate) fn run_sync_stage<T, F>(
    reporter: StageReporter,
    stage: SessionStage,
    operation: F,
) -> Result<T, StageFailure>
where
    F: FnOnce() -> Result<T, String>,
{
    let span = reporter.enter(stage, 1);
    match operation() {
        Ok(value) => {
            span.finish(StageResultClass::Success);
            Ok(value)
        }
        Err(error) => {
            span.finish(StageResultClass::NativeError);
            Err(StageFailure::new(
                stage,
                StageResultClass::NativeError,
                format!("Windows WinRT {} failed: {error}", stage.name()),
            ))
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FirstFrameKind {
    Ecg,
    Acc,
}

pub(crate) struct FirstFrameStages {
    reporter: StageReporter,
    ecg: FrameStage,
    acc: FrameStage,
}

#[derive(Default)]
struct FrameStage {
    span: Option<StageSpan>,
    completed: bool,
    started: bool,
}

impl FirstFrameStages {
    pub(crate) fn new(reporter: StageReporter) -> Self {
        Self {
            reporter,
            ecg: FrameStage::default(),
            acc: FrameStage::default(),
        }
    }

    pub(crate) fn begin(&mut self, kind: FirstFrameKind) {
        let (stage, state) = match kind {
            FirstFrameKind::Ecg => (SessionStage::FirstEcgFrame, &mut self.ecg),
            FirstFrameKind::Acc => (SessionStage::FirstAccFrame, &mut self.acc),
        };
        if state.started {
            return;
        }
        state.started = true;
        let span = self.reporter.enter(stage, 1);
        if state.completed {
            span.finish(StageResultClass::Success);
        } else {
            state.span = Some(span);
        }
    }

    pub(crate) fn observe(&mut self, kind: FirstFrameKind) {
        let state = match kind {
            FirstFrameKind::Ecg => &mut self.ecg,
            FirstFrameKind::Acc => &mut self.acc,
        };
        state.completed = true;
        if let Some(span) = state.span.take() {
            span.finish(StageResultClass::Success);
        }
    }

    pub(crate) fn finish(&mut self, kind: FirstFrameKind, class: StageResultClass) {
        let state = match kind {
            FirstFrameKind::Ecg => &mut self.ecg,
            FirstFrameKind::Acc => &mut self.acc,
        };
        if let Some(span) = state.span.take() {
            span.finish(class);
        }
    }

    pub(crate) fn finish_pending(&mut self, class: StageResultClass) {
        self.finish(FirstFrameKind::Ecg, class);
        self.finish(FirstFrameKind::Acc, class);
    }

    #[cfg(test)]
    fn saw_ecg(&self) -> bool {
        self.ecg.completed
    }

    #[cfg(test)]
    fn saw_acc(&self) -> bool {
        self.acc.completed
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubscriptionKind {
    HeartRate,
    PmdControl,
    PmdData,
}

#[derive(Default)]
pub(crate) struct SessionCleanup {
    subscriptions: Vec<SubscriptionKind>,
    started: bool,
}

pub(crate) struct CleanupPlan {
    pub(crate) subscriptions: Vec<SubscriptionKind>,
    pub(crate) close_session: bool,
}

impl SessionCleanup {
    pub(crate) fn record_subscription(&mut self, subscription: SubscriptionKind) {
        debug_assert!(!self.started);
        self.subscriptions.push(subscription);
    }

    pub(crate) fn begin(&mut self) -> Option<CleanupPlan> {
        if self.started {
            return None;
        }
        self.started = true;
        let subscriptions = self.subscriptions.iter().rev().copied().collect();
        Some(CleanupPlan {
            subscriptions,
            close_session: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::{pending, ready},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::*;

    #[derive(Clone, Default)]
    struct FakeControl {
        cancelled: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    impl StageControl for FakeControl {
        fn cancel(&self) -> Result<(), String> {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn close(&self) -> Result<(), String> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone, Default)]
    struct RetainedSuccessControl(FakeControl);

    impl StageControl for RetainedSuccessControl {
        fn cancel(&self) -> Result<(), String> {
            self.0.cancel()
        }

        fn close(&self) -> Result<(), String> {
            self.0.close()
        }

        fn close_after_success(&self) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn every_stage_covers_success_native_error_timeout_and_cancellation() {
        for stage in SessionStage::ALL {
            let reporter = StageReporter::new(false, None);

            let (_cancel, mut cancelled) = watch::channel(false);
            let control = FakeControl::default();
            assert_eq!(
                run_controlled_stage(
                    reporter,
                    stage,
                    1,
                    Duration::from_secs(1),
                    &mut cancelled,
                    ready(Ok::<_, String>(())),
                    control.clone(),
                )
                .await,
                Ok(())
            );
            assert_eq!(control.cancelled.load(Ordering::SeqCst), 0);
            assert_eq!(control.closed.load(Ordering::SeqCst), 1);

            let (_cancel, mut cancelled) = watch::channel(false);
            let control = FakeControl::default();
            let error = run_controlled_stage(
                reporter,
                stage,
                1,
                Duration::from_secs(1),
                &mut cancelled,
                ready(Err::<(), _>("synthetic native error".to_string())),
                control.clone(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.class(), StageResultClass::NativeError);
            assert_eq!(control.cancelled.load(Ordering::SeqCst), 0);
            assert_eq!(control.closed.load(Ordering::SeqCst), 1);

            let (_cancel, mut cancelled) = watch::channel(false);
            let control = FakeControl::default();
            let error = run_controlled_stage(
                reporter,
                stage,
                1,
                Duration::from_millis(1),
                &mut cancelled,
                pending::<Result<(), String>>(),
                control.clone(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.class(), StageResultClass::TimeoutCancelled);
            assert_eq!(control.cancelled.load(Ordering::SeqCst), 1);
            assert_eq!(control.closed.load(Ordering::SeqCst), 1);

            let (cancel, mut cancelled) = watch::channel(false);
            cancel.send_replace(true);
            let control = FakeControl::default();
            let error = run_controlled_stage(
                reporter,
                stage,
                1,
                Duration::from_secs(1),
                &mut cancelled,
                pending::<Result<(), String>>(),
                control.clone(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.class(), StageResultClass::Cancelled);
            assert_eq!(control.cancelled.load(Ordering::SeqCst), 1);
            assert_eq!(control.closed.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn retained_success_skips_only_success_close() {
        let reporter = StageReporter::new(false, None);
        let stage = SessionStage::PmdDataNotification;

        let (_cancel, mut cancelled) = watch::channel(false);
        let control = RetainedSuccessControl::default();
        assert_eq!(
            run_controlled_stage(
                reporter,
                stage,
                1,
                Duration::from_secs(1),
                &mut cancelled,
                ready(Ok::<_, String>(())),
                control.clone(),
            )
            .await,
            Ok(())
        );
        assert_eq!(control.0.cancelled.load(Ordering::SeqCst), 0);
        assert_eq!(control.0.closed.load(Ordering::SeqCst), 0);

        let (_cancel, mut cancelled) = watch::channel(false);
        let control = RetainedSuccessControl::default();
        let error = run_controlled_stage(
            reporter,
            stage,
            1,
            Duration::from_secs(1),
            &mut cancelled,
            ready(Err::<(), _>("synthetic native error".to_string())),
            control.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.class(), StageResultClass::NativeError);
        assert_eq!(control.0.cancelled.load(Ordering::SeqCst), 0);
        assert_eq!(control.0.closed.load(Ordering::SeqCst), 1);

        let (_cancel, mut cancelled) = watch::channel(false);
        let control = RetainedSuccessControl::default();
        let error = run_controlled_stage(
            reporter,
            stage,
            1,
            Duration::from_millis(1),
            &mut cancelled,
            pending::<Result<(), String>>(),
            control.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.class(), StageResultClass::TimeoutCancelled);
        assert_eq!(control.0.cancelled.load(Ordering::SeqCst), 1);
        assert_eq!(control.0.closed.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn first_frame_stages_keep_missing_streams_distinct() {
        let mut stages = FirstFrameStages::new(StageReporter::new(false, None));
        stages.begin(FirstFrameKind::Ecg);
        stages.observe(FirstFrameKind::Ecg);
        assert!(stages.saw_ecg());
        assert!(!stages.saw_acc());
        stages.begin(FirstFrameKind::Acc);
        stages.finish_pending(StageResultClass::Timeout);

        let mut stages = FirstFrameStages::new(StageReporter::new(false, None));
        stages.begin(FirstFrameKind::Acc);
        stages.observe(FirstFrameKind::Acc);
        assert!(!stages.saw_ecg());
        assert!(stages.saw_acc());
        stages.finish_pending(StageResultClass::Timeout);
    }

    #[test]
    fn partial_subscription_cleanup_is_reverse_order_and_exactly_once() {
        let mut cleanup = SessionCleanup::default();
        cleanup.record_subscription(SubscriptionKind::HeartRate);
        cleanup.record_subscription(SubscriptionKind::PmdControl);
        let plan = cleanup.begin().expect("first cleanup owns rollback");
        assert_eq!(
            plan.subscriptions,
            vec![SubscriptionKind::PmdControl, SubscriptionKind::HeartRate]
        );
        assert!(plan.close_session);
        assert!(cleanup.begin().is_none());
    }
}
