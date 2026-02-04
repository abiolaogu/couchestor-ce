//! Event Publisher Adapter
//!
//! Implements the `EventPublisher` port with various backends.

use async_trait::async_trait;
use tracing::{debug, info};

use crate::domain::events::DomainEvent;
use crate::domain::ports::EventPublisher;
use crate::error::Result;

/// Logging-based event publisher.
///
/// Publishes domain events to the tracing/logging system.
/// Useful for development, debugging, and audit trails.
#[allow(dead_code)]
#[derive(Debug, Clone, Default)]
pub struct LoggingEventPublisher {
    /// Whether to log events at info level (true) or debug level (false)
    info_level: bool,
}

#[allow(dead_code)]
impl LoggingEventPublisher {
    /// Create a new logging event publisher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a publisher that logs at info level.
    pub fn info_level() -> Self {
        Self { info_level: true }
    }

    /// Create a publisher that logs at debug level.
    pub fn debug_level() -> Self {
        Self { info_level: false }
    }
}

#[async_trait]
impl EventPublisher for LoggingEventPublisher {
    async fn publish(&self, event: DomainEvent) -> Result<()> {
        let event_type = event.event_type();
        let json = serde_json::to_string(&event).unwrap_or_else(|_| format!("{:?}", event));

        if self.info_level {
            info!(event_type = %event_type, event = %json, "Domain event");
        } else {
            debug!(event_type = %event_type, event = %json, "Domain event");
        }

        Ok(())
    }

    async fn publish_all(&self, events: Vec<DomainEvent>) -> Result<()> {
        for event in events {
            self.publish(event).await?;
        }
        Ok(())
    }
}

/// In-memory event collector for testing.
///
/// Collects events in memory for later inspection during tests.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct InMemoryEventCollector {
    events: parking_lot::RwLock<Vec<DomainEvent>>,
}

#[allow(dead_code)]
impl InMemoryEventCollector {
    /// Create a new in-memory event collector.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get all collected events.
    pub fn events(&self) -> Vec<DomainEvent> {
        self.events.read().clone()
    }

    /// Get the count of collected events.
    pub fn len(&self) -> usize {
        self.events.read().len()
    }

    /// Check if there are no events.
    pub fn is_empty(&self) -> bool {
        self.events.read().is_empty()
    }

    /// Clear all collected events.
    pub fn clear(&self) {
        self.events.write().clear();
    }

    /// Get events of a specific type.
    pub fn events_of_type(&self, event_type: &str) -> Vec<DomainEvent> {
        self.events
            .read()
            .iter()
            .filter(|e| e.event_type() == event_type)
            .cloned()
            .collect()
    }
}

#[async_trait]
impl EventPublisher for InMemoryEventCollector {
    async fn publish(&self, event: DomainEvent) -> Result<()> {
        self.events.write().push(event);
        Ok(())
    }

    async fn publish_all(&self, events: Vec<DomainEvent>) -> Result<()> {
        self.events.write().extend(events);
        Ok(())
    }
}

/// Composite event publisher that publishes to multiple backends.
#[allow(dead_code)]
#[derive(Default)]
pub struct CompositeEventPublisher {
    publishers: Vec<Box<dyn EventPublisher>>,
}

#[allow(dead_code)]
impl CompositeEventPublisher {
    /// Create a new composite publisher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a publisher to the composite.
    pub fn with_publisher<P: EventPublisher + 'static>(mut self, publisher: P) -> Self {
        self.publishers.push(Box::new(publisher));
        self
    }
}

impl std::fmt::Debug for CompositeEventPublisher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompositeEventPublisher")
            .field("publisher_count", &self.publishers.len())
            .finish()
    }
}

#[async_trait]
impl EventPublisher for CompositeEventPublisher {
    async fn publish(&self, event: DomainEvent) -> Result<()> {
        for publisher in &self.publishers {
            publisher.publish(event.clone()).await?;
        }
        Ok(())
    }

    async fn publish_all(&self, events: Vec<DomainEvent>) -> Result<()> {
        for publisher in &self.publishers {
            publisher.publish_all(events.clone()).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ports::StorageTier;

    #[tokio::test]
    async fn test_logging_publisher() {
        let publisher = LoggingEventPublisher::new();
        let event = DomainEvent::volume_created("test-vol", 1024, StorageTier::Hot);

        // Should not panic
        publisher.publish(event).await.unwrap();
    }

    #[tokio::test]
    async fn test_in_memory_collector() {
        let collector = InMemoryEventCollector::new();

        assert!(collector.is_empty());

        let event1 = DomainEvent::volume_created("vol-1", 1024, StorageTier::Hot);
        let event2 = DomainEvent::volume_created("vol-2", 2048, StorageTier::Cold);

        collector.publish(event1).await.unwrap();
        collector.publish(event2).await.unwrap();

        assert_eq!(collector.len(), 2);

        let created_events = collector.events_of_type("VolumeCreated");
        assert_eq!(created_events.len(), 2);

        collector.clear();
        assert!(collector.is_empty());
    }

    #[tokio::test]
    async fn test_composite_publisher() {
        let composite =
            CompositeEventPublisher::new().with_publisher(LoggingEventPublisher::debug_level());

        let event = DomainEvent::volume_created("test", 1024, StorageTier::Hot);
        composite.publish(event).await.unwrap();
    }
}
