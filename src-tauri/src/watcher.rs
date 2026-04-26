use crate::types::WatchStatus;

pub trait Watcher: Send + Sync {
    fn status(&self) -> WatchStatus;
}
