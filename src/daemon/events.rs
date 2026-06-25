use crate::model::ServerEvent;
use tokio::sync::broadcast;

pub type EventSender = broadcast::Sender<ServerEvent>;

pub fn new_event_channel(capacity: usize) -> (EventSender, broadcast::Receiver<ServerEvent>) {
    broadcast::channel(capacity)
}
