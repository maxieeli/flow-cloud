use casbin::{CoreApi, Enforcer, MgmtApi};
use tokio::sync::RwLock;
use crate::biz::casbin::metrics::MetricsCalState;

pub struct AFEnforcer<T> {
    enforcer: RwLock<Enforcer>,
    pub(crate) metrics_state: MetricsCalState,
    enforce_group: T,
}