//! Strongly-typed identifier newtypes for Richter domain objects.
//!
//! Each identifier wraps a [`uuid::Uuid`] and provides `Deref`, `Display`,
//! `FromStr`, and bidirectional `From<Uuid>` conversions without permitting
//! accidental cross-type usage.

crate::define_id!(RepoId);
crate::define_id!(WorktreeId);
crate::define_id!(AgentId);
crate::define_id!(SessionId);
crate::define_id!(RunId);
crate::define_id!(EventId);
crate::define_id!(DecisionId);
crate::define_id!(LeaseId);
crate::define_id!(ModelCallId);
crate::define_id!(CommandInvocationId);
crate::define_id!(SubscriberId);
crate::define_id!(ArtifactId);
crate::define_id!(CacheEntryId);
crate::define_id!(ImportantEventId);
crate::define_id!(PluginManifestId);
crate::define_id!(SettingId);
