use std::collections::{BTreeSet, HashMap};

use costs::{
    cost_return_on_error, cost_return_on_error_no_add, CostResult, CostsExt, OperationCost,
};
use merk::{Merk, MerkOptions};
use storage::{
    rocksdb_storage::{
        PrefixedRocksDbBatchTransactionContext, PrefixedRocksDbStorageContext,
        PrefixedRocksDbTransactionContext,
    },
    Storage, StorageBatch, StorageContext,
};

use crate::{
    batch::{key_info::KeyInfo, GroveDbOp, KeyInfoPath, Op},
    util::{
        merk_optional_tx, storage_context_optional_tx, storage_context_with_parent_optional_tx,
    },
    Element, Error, GroveDb, Transaction, TransactionArg,
};

#[derive(Clone)]
pub struct DeleteOptions {
    pub allow_deleting_non_empty_trees: bool,
    pub deleting_non_empty_trees_returns_error: bool,
    pub base_root_storage_is_free: bool,
}

impl Default for DeleteOptions {
    fn default() -> Self {
        DeleteOptions {
            allow_deleting_non_empty_trees: false,
            deleting_non_empty_trees_returns_error: true,
            base_root_storage_is_free: true,
        }
    }
}

impl DeleteOptions {
    fn as_merk_options(&self) -> MerkOptions {
        MerkOptions {
            base_root_storage_is_free: self.base_root_storage_is_free,
        }
    }
}

impl GroveDb {
    /// Delete up tree while empty will delete nodes while they are empty up a
    /// tree.
    pub fn delete_up_tree_while_empty<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        stop_path_height: Option<u16>,
        validate: bool,
        transaction: TransactionArg,
    ) -> CostResult<u16, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();
        let mut batch_operations: Vec<GroveDbOp> = Vec::new();
        let path_iter = path.into_iter();
        let path_len = path_iter.len();
        let maybe_ops = cost_return_on_error!(
            &mut cost,
            self.add_delete_operations_for_delete_up_tree_while_empty(
                path_iter,
                key,
                stop_path_height,
                validate,
                &mut batch_operations,
                transaction
            )
        );

        let ops = cost_return_on_error_no_add!(
            &cost,
            if let Some(stop_path_height) = stop_path_height {
                maybe_ops.ok_or(Error::DeleteUpTreeStopHeightMoreThanInitialPathSize(
                    format!(
                        "stop path height {} more than path size of {}",
                        stop_path_height, path_len
                    ),
                ))
            } else {
                maybe_ops.ok_or(Error::CorruptedCodeExecution(
                    "stop path height not set, but still not deleting element",
                ))
            }
        );
        let ops_len = ops.len();
        self.apply_batch(ops, None, transaction)
            .map_ok(|_| ops_len as u16)
    }

    pub fn delete_operations_for_delete_up_tree_while_empty<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        stop_path_height: Option<u16>,
        validate: bool,
        mut current_batch_operations: Vec<GroveDbOp>,
        transaction: TransactionArg,
    ) -> CostResult<Option<Vec<GroveDbOp>>, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        self.add_delete_operations_for_delete_up_tree_while_empty(
            path,
            key,
            stop_path_height,
            validate,
            &mut current_batch_operations,
            transaction,
        )
    }

    pub fn add_delete_operations_for_delete_up_tree_while_empty<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        stop_path_height: Option<u16>,
        validate: bool,
        current_batch_operations: &mut Vec<GroveDbOp>,
        transaction: TransactionArg,
    ) -> CostResult<Option<Vec<GroveDbOp>>, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let mut path_iter = path.into_iter();
        if let Some(stop_path_height) = stop_path_height {
            if stop_path_height == path_iter.clone().len() as u16 {
                return Ok(None).wrap_with_cost(cost);
            }
        }
        if validate {
            cost_return_on_error!(
                &mut cost,
                self.check_subtree_exists_path_not_found(path_iter.clone(), transaction)
            );
        }
        if let Some(delete_operation_this_level) = cost_return_on_error!(
            &mut cost,
            self.delete_operation_for_delete_internal(
                path_iter.clone(),
                key,
                DeleteOptions {
                    allow_deleting_non_empty_trees: false,
                    deleting_non_empty_trees_returns_error: false,
                    ..Default::default()
                },
                validate,
                current_batch_operations,
                transaction,
            )
        ) {
            let mut delete_operations = vec![delete_operation_this_level.clone()];
            if let Some(last) = path_iter.next_back() {
                current_batch_operations.push(delete_operation_this_level);
                if let Some(mut delete_operations_upper_level) = cost_return_on_error!(
                    &mut cost,
                    self.add_delete_operations_for_delete_up_tree_while_empty(
                        path_iter,
                        last,
                        stop_path_height,
                        validate,
                        current_batch_operations,
                        transaction,
                    )
                ) {
                    delete_operations.append(&mut delete_operations_upper_level);
                }
            }
            Ok(Some(delete_operations)).wrap_with_cost(cost)
        } else {
            Ok(None).wrap_with_cost(cost)
        }
    }

    pub fn delete<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        options: Option<DeleteOptions>,
        transaction: TransactionArg,
    ) -> CostResult<(), Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let options = options.unwrap_or_default();
        self.delete_internal(path, key, options, transaction)
            .map_ok(|_| ())
    }

    pub fn delete_if_empty_tree<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        transaction: TransactionArg,
    ) -> CostResult<bool, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let options = DeleteOptions {
            allow_deleting_non_empty_trees: false,
            deleting_non_empty_trees_returns_error: false,
            ..Default::default()
        };
        self.delete_internal(path, key, options, transaction)
    }

    pub fn delete_operation_for_delete_internal<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        options: DeleteOptions,
        validate: bool,
        current_batch_operations: &[GroveDbOp],
        transaction: TransactionArg,
    ) -> CostResult<Option<GroveDbOp>, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let path_iter = path.into_iter();
        if path_iter.len() == 0 {
            // Attempt to delete a root tree leaf
            Err(Error::InvalidPath(
                "root tree leaves currently cannot be deleted".to_owned(),
            ))
            .wrap_with_cost(cost)
        } else {
            if validate {
                cost_return_on_error!(
                    &mut cost,
                    self.check_subtree_exists_path_not_found(path_iter.clone(), transaction)
                );
            }
            let element = cost_return_on_error!(
                &mut cost,
                self.get_raw(path_iter.clone(), key.as_ref(), transaction)
            );

            if let Element::Tree(..) = element {
                let subtree_merk_path = path_iter.clone().chain(std::iter::once(key));
                let subtree_merk_path_vec = subtree_merk_path
                    .clone()
                    .map(|x| x.to_vec())
                    .collect::<Vec<Vec<u8>>>();
                // TODO: may be a bug
                let _subtrees_paths = cost_return_on_error!(
                    &mut cost,
                    self.find_subtrees(subtree_merk_path.clone(), transaction)
                );
                let batch_deleted_keys = current_batch_operations
                    .iter()
                    .filter_map(|op| match op.op {
                        Op::Delete | Op::DeleteTree => {
                            // todo: to_path clones (best to figure out how to compare without
                            // cloning)
                            if op.path.to_path() == subtree_merk_path_vec {
                                Some(op.key.as_slice())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    })
                    .collect::<BTreeSet<&[u8]>>();
                let mut is_empty = merk_optional_tx!(
                    &mut cost,
                    self.db,
                    subtree_merk_path,
                    transaction,
                    subtree,
                    {
                        subtree
                            .is_empty_tree_except(batch_deleted_keys)
                            .unwrap_add_cost(&mut cost)
                    }
                );

                // If there is any current batch operation that is inserting something in this
                // tree then it is not empty either
                is_empty &= !current_batch_operations.iter().any(|op| match op.op {
                    Op::Delete | Op::DeleteTree => false,
                    // todo: fix for to_path (it clones)
                    _ => op.path.to_path() == subtree_merk_path_vec,
                });

                let result = if !options.allow_deleting_non_empty_trees && !is_empty {
                    if options.deleting_non_empty_trees_returns_error {
                        Err(Error::DeletingNonEmptyTree(
                            "trying to do a delete operation for a non empty tree, but options \
                             not allowing this",
                        ))
                    } else {
                        Ok(None)
                    }
                } else if is_empty {
                    Ok(Some(GroveDbOp::delete_tree_run_op(
                        path_iter.map(|x| x.to_vec()).collect(),
                        key.to_vec(),
                    )))
                } else {
                    Err(Error::NotSupported(
                        "deletion operation for non empty tree not currently supported",
                    ))
                };
                result.wrap_with_cost(cost)
            } else {
                Ok(Some(GroveDbOp::delete_run_op(
                    path_iter.map(|x| x.to_vec()).collect(),
                    key.to_vec(),
                )))
                .wrap_with_cost(cost)
            }
        }
    }

    pub fn worst_case_delete_operation_for_delete_internal<'p, 'db, S: Storage<'db>, P>(
        &self,
        path: &KeyInfoPath,
        key: &KeyInfo,
        validate: bool,
        max_element_size: u32,
    ) -> CostResult<Option<GroveDbOp>, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        if path.len() == 0 {
            // Attempt to delete a root tree leaf
            Err(Error::InvalidPath(
                "root tree leaves currently cannot be deleted".to_owned(),
            ))
            .wrap_with_cost(cost)
        } else {
            if validate {
                GroveDb::add_worst_case_get_merk_at_path::<S>(&mut cost, path);
            }
            GroveDb::add_worst_case_get_raw_cost::<S>(&mut cost, path, key, max_element_size);
            Ok(Some(GroveDbOp::delete_worst_case_op(
                path.clone(),
                key.clone(),
            )))
            .wrap_with_cost(cost)
        }
    }

    fn delete_internal<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        options: DeleteOptions,
        transaction: TransactionArg,
    ) -> CostResult<bool, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        if let Some(transaction) = transaction {
            self.delete_internal_on_transaction(path, key, options, transaction)
        } else {
            self.delete_internal_without_transaction(path, key, options)
        }
    }

    fn delete_internal_on_transaction<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        options: DeleteOptions,
        transaction: &Transaction,
    ) -> CostResult<bool, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let path_iter = path.into_iter();
        let element = cost_return_on_error!(
            &mut cost,
            self.get_raw(path_iter.clone(), key.as_ref(), Some(transaction))
        );
        let mut subtree_to_delete_from = cost_return_on_error!(
            &mut cost,
            self.open_transactional_merk_at_path(path_iter.clone(), transaction)
        );
        if let Element::Tree(..) = element {
            let subtree_merk_path = path_iter.clone().chain(std::iter::once(key));

            let subtree_of_tree_we_are_deleting = cost_return_on_error!(
                &mut cost,
                self.open_transactional_merk_at_path(subtree_merk_path.clone(), transaction)
            );
            let is_empty = subtree_of_tree_we_are_deleting
                .is_empty_tree()
                .unwrap_add_cost(&mut cost);

            if !options.allow_deleting_non_empty_trees && !is_empty {
                return if options.deleting_non_empty_trees_returns_error {
                    Err(Error::DeletingNonEmptyTree(
                        "trying to do a delete operation for a non empty tree, but options not \
                         allowing this",
                    ))
                    .wrap_with_cost(cost)
                } else {
                    Ok(false).wrap_with_cost(cost)
                };
            } else {
                if !is_empty {
                    let storage_batch = StorageBatch::new();
                    let subtrees_paths = cost_return_on_error!(
                        &mut cost,
                        self.find_subtrees(subtree_merk_path, Some(transaction))
                    );
                    for subtree_path in subtrees_paths {
                        let mut storage = self
                            .db
                            .get_batch_transactional_storage_context(
                                subtree_path.iter().map(|x| x.as_slice()),
                                &storage_batch,
                                transaction,
                            )
                            .unwrap_add_cost(&mut cost);

                        cost_return_on_error!(
                            &mut cost,
                            storage.clear().map_err(|e| {
                                Error::CorruptedData(format!(
                                    "unable to cleanup tree from storage: {}",
                                    e
                                ))
                            })
                        );
                    }
                    let storage = self
                        .db
                        .get_batch_transactional_storage_context(
                            path_iter.clone(),
                            &storage_batch,
                            transaction,
                        )
                        .unwrap_add_cost(&mut cost);

                    let mut merk_to_delete_tree_from = cost_return_on_error!(
                        &mut cost,
                        Merk::open_layered_with_root_key(
                            storage,
                            subtree_to_delete_from.root_key()
                        )
                        .map_err(|_| {
                            Error::CorruptedData(
                                "cannot open a subtree with given root key".to_owned(),
                            )
                        })
                    );
                    // We are deleting a tree, a tree uses 3 bytes
                    cost_return_on_error!(
                        &mut cost,
                        Element::delete(
                            &mut merk_to_delete_tree_from,
                            &key,
                            Some(options.as_merk_options()),
                            true,
                        )
                    );
                    let mut merk_cache: HashMap<
                        Vec<Vec<u8>>,
                        Merk<PrefixedRocksDbBatchTransactionContext>,
                    > = HashMap::default();
                    merk_cache.insert(
                        path_iter.clone().map(|k| k.to_vec()).collect(),
                        merk_to_delete_tree_from,
                    );
                    cost_return_on_error!(
                        &mut cost,
                        self.propagate_changes_with_batch_transaction(
                            &storage_batch,
                            merk_cache,
                            path_iter,
                            transaction
                        )
                    );
                    cost_return_on_error_no_add!(
                        &cost,
                        self.db
                            .commit_multi_context_batch(storage_batch, Some(transaction))
                            .unwrap_add_cost(&mut cost)
                            .map_err(|e| e.into())
                    );
                } else {
                    // We are deleting a tree, a tree uses 3 bytes
                    cost_return_on_error!(
                        &mut cost,
                        Element::delete(
                            &mut subtree_to_delete_from,
                            &key,
                            Some(options.as_merk_options()),
                            true,
                        )
                    );
                    let mut merk_cache: HashMap<
                        Vec<Vec<u8>>,
                        Merk<PrefixedRocksDbTransactionContext>,
                    > = HashMap::default();
                    merk_cache.insert(
                        path_iter.clone().map(|k| k.to_vec()).collect(),
                        subtree_to_delete_from,
                    );
                    cost_return_on_error!(
                        &mut cost,
                        self.propagate_changes_with_transaction(merk_cache, path_iter, transaction)
                    );
                }
            }
        } else {
            cost_return_on_error!(
                &mut cost,
                Element::delete(
                    &mut subtree_to_delete_from,
                    &key,
                    Some(options.as_merk_options()),
                    false
                )
            );
            let mut merk_cache: HashMap<Vec<Vec<u8>>, Merk<PrefixedRocksDbTransactionContext>> =
                HashMap::default();
            merk_cache.insert(
                path_iter.clone().map(|k| k.to_vec()).collect(),
                subtree_to_delete_from,
            );
            cost_return_on_error!(
                &mut cost,
                self.propagate_changes_with_transaction(merk_cache, path_iter, transaction)
            );
        }

        Ok(true).wrap_with_cost(cost)
    }

    fn delete_internal_without_transaction<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        options: DeleteOptions,
    ) -> CostResult<bool, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let path_iter = path.into_iter();
        let element = cost_return_on_error!(
            &mut cost,
            self.get_raw(path_iter.clone(), key.as_ref(), None)
        );
        let mut merk_cache: HashMap<Vec<Vec<u8>>, Merk<PrefixedRocksDbStorageContext>> =
            HashMap::default();
        let mut subtree_to_delete_from: Merk<PrefixedRocksDbStorageContext> = cost_return_on_error!(
            &mut cost,
            self.open_non_transactional_merk_at_path(path_iter.clone())
        );
        if let Element::Tree(..) = element {
            let subtree_merk_path = path_iter.clone().chain(std::iter::once(key));
            let subtree_of_tree_we_are_deleting = cost_return_on_error!(
                &mut cost,
                self.open_non_transactional_merk_at_path(subtree_merk_path.clone())
            );
            let is_empty = subtree_of_tree_we_are_deleting
                .is_empty_tree()
                .unwrap_add_cost(&mut cost);

            if !options.allow_deleting_non_empty_trees && !is_empty {
                return if options.deleting_non_empty_trees_returns_error {
                    Err(Error::DeletingNonEmptyTree(
                        "trying to do a delete operation for a non empty tree, but options not \
                         allowing this",
                    ))
                    .wrap_with_cost(cost)
                } else {
                    Ok(false).wrap_with_cost(cost)
                };
            } else {
                if !is_empty {
                    let subtrees_paths = cost_return_on_error!(
                        &mut cost,
                        self.find_subtrees(subtree_merk_path, None)
                    );
                    // TODO: dumb traversal should not be tolerated
                    for subtree_path in subtrees_paths.into_iter().rev() {
                        let mut inner_subtree_to_delete_from = cost_return_on_error!(
                            &mut cost,
                            self.open_non_transactional_merk_at_path(
                                subtree_path.iter().map(|x| x.as_slice())
                            )
                        );
                        cost_return_on_error!(
                            &mut cost,
                            inner_subtree_to_delete_from.clear().map_err(|e| {
                                Error::CorruptedData(format!(
                                    "unable to cleanup tree from storage: {}",
                                    e
                                ))
                            })
                        );
                    }
                }
                cost_return_on_error!(
                    &mut cost,
                    Element::delete(
                        &mut subtree_to_delete_from,
                        &key,
                        Some(options.as_merk_options()),
                        true,
                    )
                );
            }
        } else {
            cost_return_on_error!(
                &mut cost,
                Element::delete(
                    &mut subtree_to_delete_from,
                    &key,
                    Some(options.as_merk_options()),
                    false,
                )
            );
        }
        merk_cache.insert(
            path_iter.clone().map(|k| k.to_vec()).collect(),
            subtree_to_delete_from,
        );
        cost_return_on_error!(
            &mut cost,
            self.propagate_changes_without_transaction(merk_cache, path_iter)
        );

        Ok(true).wrap_with_cost(cost)
    }

    // TODO: dumb traversal should not be tolerated
    /// Finds keys which are trees for a given subtree recursively.
    /// One element means a key of a `merk`, n > 1 elements mean relative path
    /// for a deeply nested subtree.
    pub(crate) fn find_subtrees<'p, P>(
        &self,
        path: P,
        transaction: TransactionArg,
    ) -> CostResult<Vec<Vec<Vec<u8>>>, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
    {
        let mut cost = OperationCost::default();

        // TODO: remove conversion to vec;
        // However, it's not easy for a reason:
        // new keys to enqueue are taken from raw iterator which returns Vec<u8>;
        // changing that to slice is hard as cursor should be moved for next iteration
        // which requires exclusive (&mut) reference, also there is no guarantee that
        // slice which points into storage internals will remain valid if raw
        // iterator got altered so why that reference should be exclusive;

        let mut queue: Vec<Vec<Vec<u8>>> = vec![path.into_iter().map(|x| x.to_vec()).collect()];
        let mut result: Vec<Vec<Vec<u8>>> = queue.clone();

        while let Some(q) = queue.pop() {
            // Get the correct subtree with q_ref as path
            let path_iter = q.iter().map(|x| x.as_slice());
            storage_context_optional_tx!(self.db, path_iter.clone(), transaction, storage, {
                let storage = storage.unwrap_add_cost(&mut cost);
                let mut raw_iter = Element::iterator(storage.raw_iter()).unwrap_add_cost(&mut cost);
                while let Some((key, value)) = cost_return_on_error!(&mut cost, raw_iter.next()) {
                    if let Element::Tree(..) = value {
                        let mut sub_path = q.clone();
                        sub_path.push(key.to_vec());
                        queue.push(sub_path.clone());
                        result.push(sub_path);
                    }
                }
            })
        }
        Ok(result).wrap_with_cost(cost)
    }

    pub fn worst_case_deletion_cost<'p, P>(
        &self,
        _path: P,
        key: &'p [u8],
        max_element_size: u32,
    ) -> OperationCost
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: ExactSizeIterator + DoubleEndedIterator + Clone,
    {
        let mut cost = OperationCost::default();
        GroveDb::add_worst_case_delete_cost(
            &mut cost,
            // path,
            key.len() as u32,
            max_element_size,
        );
        cost
    }
}

#[cfg(test)]
mod tests {
    use costs::{
        storage_cost::{removal::StorageRemovedBytes::BasicStorageRemoval, StorageCost},
        OperationCost,
    };
    use pretty_assertions::assert_eq;

    use crate::{
        operations::delete::DeleteOptions,
        tests::{make_empty_grovedb, make_test_grovedb, ANOTHER_TEST_LEAF, TEST_LEAF},
        Element, Error,
    };

    #[test]
    fn test_empty_subtree_deletion_without_transaction() {
        let _element = Element::new_item(b"ayy".to_vec());
        let db = make_test_grovedb();
        // Insert some nested subtrees
        db.insert([TEST_LEAF], b"key1", Element::empty_tree(), None, None)
            .unwrap()
            .expect("successful subtree 1 insert");
        db.insert([TEST_LEAF], b"key4", Element::empty_tree(), None, None)
            .unwrap()
            .expect("successful subtree 3 insert");

        let root_hash = db.root_hash(None).unwrap().unwrap();
        db.delete([TEST_LEAF], b"key1", None, None)
            .unwrap()
            .expect("unable to delete subtree");
        assert!(matches!(
            db.get([TEST_LEAF, b"key1", b"key2"], b"key3", None)
                .unwrap(),
            Err(Error::PathNotFound(_))
        ));
        // assert_eq!(db.subtrees.len().unwrap(), 3); // TEST_LEAF, ANOTHER_TEST_LEAF
        // TEST_LEAF.key4 stay
        assert!(db.get([], TEST_LEAF, None).unwrap().is_ok());
        assert!(db.get([], ANOTHER_TEST_LEAF, None).unwrap().is_ok());
        assert!(db.get([TEST_LEAF], b"key4", None).unwrap().is_ok());
        assert_ne!(root_hash, db.root_hash(None).unwrap().unwrap());
    }

    #[test]
    fn test_empty_subtree_deletion_with_transaction() {
        let _element = Element::new_item(b"ayy".to_vec());

        let db = make_test_grovedb();
        let transaction = db.start_transaction();

        // Insert some nested subtrees
        db.insert(
            [TEST_LEAF],
            b"key1",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree 1 insert");
        db.insert(
            [TEST_LEAF],
            b"key4",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree 3 insert");

        db.delete([TEST_LEAF], b"key1", None, Some(&transaction))
            .unwrap()
            .expect("unable to delete subtree");
        assert!(matches!(
            db.get([TEST_LEAF, b"key1", b"key2"], b"key3", Some(&transaction))
                .unwrap(),
            Err(Error::PathNotFound(_))
        ));
        transaction.commit().expect("cannot commit transaction");
        assert!(matches!(
            db.get([TEST_LEAF], b"key1", None).unwrap(),
            Err(Error::PathKeyNotFound(_))
        ));
        assert!(matches!(db.get([TEST_LEAF], b"key4", None).unwrap(), Ok(_)));
    }

    #[test]
    fn test_subtree_deletion_if_empty_with_transaction() {
        let element = Element::new_item(b"value".to_vec());
        let db = make_test_grovedb();

        let transaction = db.start_transaction();

        // Insert some nested subtrees
        db.insert(
            [TEST_LEAF],
            b"level1-A",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree insert A on level 1");
        db.insert(
            [TEST_LEAF, b"level1-A"],
            b"level2-A",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree insert A on level 2");
        db.insert(
            [TEST_LEAF, b"level1-A"],
            b"level2-B",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree insert B on level 2");
        // Insert an element into subtree
        db.insert(
            [TEST_LEAF, b"level1-A", b"level2-A"],
            b"level3-A",
            element,
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful value insert");
        db.insert(
            [TEST_LEAF],
            b"level1-B",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree insert B on level 1");

        db.commit_transaction(transaction)
            .unwrap()
            .expect("cannot commit changes");

        // Currently we have:
        // Level 1:            A
        //                    / \
        // Level 2:          A   B
        //                   |
        // Level 3:          A: value

        let transaction = db.start_transaction();

        let deleted = db
            .delete_if_empty_tree([TEST_LEAF], b"level1-A", Some(&transaction))
            .unwrap()
            .expect("unable to delete subtree");
        assert!(!deleted);

        let deleted = db
            .delete_up_tree_while_empty(
                [TEST_LEAF, b"level1-A", b"level2-A"],
                b"level3-A",
                Some(0),
                true,
                Some(&transaction),
            )
            .unwrap()
            .expect("unable to delete subtree");
        assert_eq!(deleted, 2);

        assert!(matches!(
            db.get(
                [TEST_LEAF, b"level1-A", b"level2-A"],
                b"level3-A",
                Some(&transaction)
            )
            .unwrap(),
            Err(Error::PathNotFound(_))
        ));

        assert!(matches!(
            db.get([TEST_LEAF, b"level1-A"], b"level2-A", Some(&transaction))
                .unwrap(),
            Err(Error::PathKeyNotFound(_))
        ));

        assert!(matches!(
            db.get([TEST_LEAF], b"level1-A", Some(&transaction))
                .unwrap(),
            Ok(Element::Tree(..)),
        ));
    }

    #[test]
    fn test_subtree_deletion_if_empty_without_transaction() {
        let element = Element::new_item(b"value".to_vec());
        let db = make_test_grovedb();

        // Insert some nested subtrees
        db.insert([TEST_LEAF], b"level1-A", Element::empty_tree(), None, None)
            .unwrap()
            .expect("successful subtree insert A on level 1");
        db.insert(
            [TEST_LEAF, b"level1-A"],
            b"level2-A",
            Element::empty_tree(),
            None,
            None,
        )
        .unwrap()
        .expect("successful subtree insert A on level 2");
        db.insert(
            [TEST_LEAF, b"level1-A"],
            b"level2-B",
            Element::empty_tree(),
            None,
            None,
        )
        .unwrap()
        .expect("successful subtree insert B on level 2");
        // Insert an element into subtree
        db.insert(
            [TEST_LEAF, b"level1-A", b"level2-A"],
            b"level3-A",
            element,
            None,
            None,
        )
        .unwrap()
        .expect("successful value insert");
        db.insert([TEST_LEAF], b"level1-B", Element::empty_tree(), None, None)
            .unwrap()
            .expect("successful subtree insert B on level 1");

        // Currently we have:
        // Level 1:            A
        //                    / \
        // Level 2:          A   B
        //                   |
        // Level 3:          A: value

        let deleted = db
            .delete_if_empty_tree([TEST_LEAF], b"level1-A", None)
            .unwrap()
            .expect("unable to delete subtree");
        assert!(!deleted);

        let deleted = db
            .delete_up_tree_while_empty(
                [TEST_LEAF, b"level1-A", b"level2-A"],
                b"level3-A",
                Some(0),
                true,
                None,
            )
            .unwrap()
            .expect("unable to delete subtree");
        assert_eq!(deleted, 2);

        assert!(matches!(
            db.get([TEST_LEAF, b"level1-A", b"level2-A"], b"level3-A", None,)
                .unwrap(),
            Err(Error::PathNotFound(_))
        ));

        assert!(matches!(
            db.get([TEST_LEAF, b"level1-A"], b"level2-A", None).unwrap(),
            Err(Error::PathKeyNotFound(_))
        ));

        assert!(matches!(
            db.get([TEST_LEAF], b"level1-A", None).unwrap(),
            Ok(Element::Tree(..)),
        ));
    }

    #[test]
    fn test_recurring_deletion_through_subtrees_with_transaction() {
        let element = Element::new_item(b"ayy".to_vec());

        let db = make_test_grovedb();
        let transaction = db.start_transaction();

        // Insert some nested subtrees
        db.insert(
            [TEST_LEAF],
            b"key1",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree 1 insert");
        db.insert(
            [TEST_LEAF, b"key1"],
            b"key2",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree 2 insert");

        // Insert an element into subtree
        db.insert(
            [TEST_LEAF, b"key1", b"key2"],
            b"key3",
            element,
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful value insert");
        db.insert(
            [TEST_LEAF],
            b"key4",
            Element::empty_tree(),
            None,
            Some(&transaction),
        )
        .unwrap()
        .expect("successful subtree 3 insert");

        db.delete(
            [TEST_LEAF],
            b"key1",
            Some(DeleteOptions {
                allow_deleting_non_empty_trees: true,
                deleting_non_empty_trees_returns_error: false,
                ..Default::default()
            }),
            Some(&transaction),
        )
        .unwrap()
        .expect("unable to delete subtree");
        assert!(matches!(
            db.get([TEST_LEAF, b"key1", b"key2"], b"key3", Some(&transaction))
                .unwrap(),
            Err(Error::PathNotFound(_))
        ));
        transaction.commit().expect("cannot commit transaction");
        assert!(matches!(
            db.get([TEST_LEAF], b"key1", None).unwrap(),
            Err(Error::PathKeyNotFound(_))
        ));
        db.get([TEST_LEAF], b"key4", None)
            .unwrap()
            .expect("expected to get key4");
    }

    #[test]
    fn test_recurring_deletion_through_subtrees_without_transaction() {
        let element = Element::new_item(b"ayy".to_vec());

        let db = make_test_grovedb();

        // Insert some nested subtrees
        db.insert([TEST_LEAF], b"key1", Element::empty_tree(), None, None)
            .unwrap()
            .expect("successful subtree 1 insert");
        db.insert(
            [TEST_LEAF, b"key1"],
            b"key2",
            Element::empty_tree(),
            None,
            None,
        )
        .unwrap()
        .expect("successful subtree 2 insert");

        // Insert an element into subtree
        db.insert([TEST_LEAF, b"key1", b"key2"], b"key3", element, None, None)
            .unwrap()
            .expect("successful value insert");
        db.insert([TEST_LEAF], b"key4", Element::empty_tree(), None, None)
            .unwrap()
            .expect("successful subtree 3 insert");

        db.delete(
            [TEST_LEAF],
            b"key1",
            Some(DeleteOptions {
                allow_deleting_non_empty_trees: true,
                deleting_non_empty_trees_returns_error: false,
                ..Default::default()
            }),
            None,
        )
        .unwrap()
        .expect("unable to delete subtree");
        assert!(matches!(
            db.get([TEST_LEAF, b"key1", b"key2"], b"key3", None)
                .unwrap(),
            Err(Error::PathNotFound(_))
        ));
        assert!(matches!(
            db.get([TEST_LEAF], b"key1", None).unwrap(),
            Err(Error::PathKeyNotFound(_))
        ));
        assert!(matches!(db.get([TEST_LEAF], b"key4", None).unwrap(), Ok(_)));
    }

    #[test]
    fn test_item_deletion() {
        let db = make_test_grovedb();
        let element = Element::new_item(b"ayy".to_vec());
        db.insert([TEST_LEAF], b"key", element, None, None)
            .unwrap()
            .expect("successful insert");
        let root_hash = db.root_hash(None).unwrap().unwrap();
        assert!(db.delete([TEST_LEAF], b"key", None, None).unwrap().is_ok());
        assert!(matches!(
            db.get([TEST_LEAF], b"key", None).unwrap(),
            Err(Error::PathKeyNotFound(_))
        ));
        assert_ne!(root_hash, db.root_hash(None).unwrap().unwrap());
    }

    #[test]
    fn test_one_delete_tree_item_cost() {
        let db = make_empty_grovedb();
        let tx = db.start_transaction();

        db.insert(
            vec![],
            b"key1",
            Element::new_item(b"cat".to_vec()),
            None,
            Some(&tx),
        )
        .cost_as_result()
        .expect("expected to insert");

        let cost = db
            .delete(vec![], b"key1", None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete");
        // Explanation for 147 storage removed bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 71
        //   1 for the flag option (but no flags)
        //   1 for the enum type item
        //   3 for "cat"
        //   1 for cat length
        // 32 for node hash
        // 32 for value hash (trees have this for free)
        // 1 byte for the value_size (required space for 70)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 71 + 39 = 147

        // Hash node calls
        // everything is empty, so no need for hashes?
        assert_eq!(
            cost,
            OperationCost {
                seek_count: 6, // todo: verify this
                storage_cost: StorageCost {
                    added_bytes: 0,
                    replaced_bytes: 0,
                    removed_bytes: BasicStorageRemoval(147)
                },
                storage_loaded_bytes: 152, // todo: verify this
                hash_node_calls: 2,        // todo: verify this
            }
        );
    }
}
