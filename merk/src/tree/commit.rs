use anyhow::Result;
use costs::storage_cost::{removal::StorageRemovedBytes, StorageCost};

use super::Tree;

/// To be used when committing a tree (writing it to a store after applying the
/// changes).
pub trait Commit {
    /// Called once per updated node when a finalized tree is to be written to a
    /// backing store or cache.
    fn write(
        &mut self,
        tree: &mut Tree,
        old_tree_cost: &impl Fn(&Vec<u8>, &Vec<u8>) -> Result<u32>,
        update_tree_value_based_on_costs: &mut impl FnMut(
            &StorageCost,
            &Vec<u8>,
            &mut Vec<u8>,
        ) -> Result<bool>,
        section_removal_bytes: &mut impl FnMut(&Vec<u8>, u32) -> Result<StorageRemovedBytes>,
    ) -> Result<()>;

    /// Called once per node after writing a node and its children. The returned
    /// tuple specifies whether or not to prune the left and right child nodes,
    /// respectively. For example, returning `(true, true)` will prune both
    /// nodes, removing them from memory.
    fn prune(&self, _tree: &Tree) -> (bool, bool) {
        (true, true)
    }
}

/// A `Commit` implementation which does not write to a store and does not prune
/// any nodes from the Tree. Useful when only keeping a tree in memory.
pub struct NoopCommit {}
impl Commit for NoopCommit {
    fn write(
        &mut self,
        _tree: &mut Tree,
        _old_tree_cost: &impl Fn(&Vec<u8>, &Vec<u8>) -> Result<u32>,
        _update_tree_value_based_on_costs: &mut impl FnMut(
            &StorageCost,
            &Vec<u8>,
            &mut Vec<u8>,
        ) -> Result<bool>,
        _section_removal_bytes: &mut impl FnMut(&Vec<u8>, u32) -> Result<StorageRemovedBytes>,
    ) -> Result<()> {
        Ok(())
    }

    fn prune(&self, _tree: &Tree) -> (bool, bool) {
        (false, false)
    }
}
