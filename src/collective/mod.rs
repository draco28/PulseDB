//! Collective management module.
//!
//! A **collective** is an isolated namespace for experiences, typically one per project.
//! Each collective has:
//! - Unique ID (UUID v7)
//! - Name and optional owner
//! - Fixed embedding dimension (set at creation)
//! - Its own vector index
//!
//! # Implementation Status
//!
//! **Types**: Defined in [`types`] (E1-S05).
//! **CRUD operations**: Coming in E1-S02 (Collective CRUD).
//!
//! Planned operations:
//! - `create_collective(name)` - Create a new collective
//! - `create_collective_with_owner(name, owner_id)` - Create with owner for multi-tenancy
//! - `get_collective(id)` - Get collective by ID
//! - `list_collectives()` - List all collectives
//! - `list_collectives_by_owner(owner_id)` - Filter by owner
//! - `get_collective_stats(id)` - Get experience count and storage size
//! - `delete_collective(id)` - Delete collective and all its data
//!
//! # Example (Future API)
//!
//! ```rust,ignore
//! use pulsedb::{PulseDB, Config};
//!
//! let db = PulseDB::open("./pulse.db", Config::default())?;
//!
//! // Create a collective
//! let id = db.create_collective("my-project")?;
//!
//! // Get collective info
//! if let Some(collective) = db.get_collective(id)? {
//!     println!("Collective: {}", collective.name);
//! }
//!
//! // List all collectives
//! for collective in db.list_collectives()? {
//!     println!("- {}: {}", collective.id, collective.name);
//! }
//!
//! // Get statistics
//! let stats = db.get_collective_stats(id)?;
//! println!("Experiences: {}", stats.experience_count);
//!
//! // Delete when no longer needed
//! db.delete_collective(id)?;
//! ```

pub mod types;

pub use types::Collective;
