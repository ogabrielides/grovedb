#[cfg(feature = "full")]
mod average_case;
#[cfg(feature = "full")]
mod query;
#[cfg(feature = "full")]
mod worst_case;

#[cfg(feature = "full")]
use std::collections::HashSet;

#[cfg(feature = "full")]
use costs::{cost_return_on_error, CostResult, CostsExt, OperationCost};
#[cfg(feature = "full")]
use merk::Merk;
#[cfg(feature = "full")]
use storage::{
    rocksdb_storage::{PrefixedRocksDbStorageContext, PrefixedRocksDbTransactionContext},
    StorageContext,
};

#[cfg(feature = "full")]
use crate::{
    reference_path::{path_from_reference_path_type, path_from_reference_qualified_path_type},
    util::storage_context_optional_tx,
    Element, Error, GroveDb, Transaction, TransactionArg,
};

#[cfg(feature = "full")]
/// Limit of possible indirections
pub const MAX_REFERENCE_HOPS: usize = 10;

#[cfg(feature = "full")]
impl GroveDb {
    pub fn get<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        transaction: TransactionArg,
    ) -> CostResult<Element, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let path_iter = path.into_iter();

        match cost_return_on_error!(&mut cost, self.get_raw(path_iter.clone(), key, transaction)) {
            Element::Reference(reference_path, ..) => {
                let path = cost_return_on_error!(
                    &mut cost,
                    path_from_reference_path_type(reference_path, path_iter, Some(key))
                        .wrap_with_cost(OperationCost::default())
                );
                self.follow_reference(path, transaction).add_cost(cost)
            }
            other => Ok(other).wrap_with_cost(cost),
        }
    }

    pub fn follow_reference(
        &self,
        mut path: Vec<Vec<u8>>,
        transaction: TransactionArg,
    ) -> CostResult<Element, Error> {
        let mut cost = OperationCost::default();

        let mut hops_left = MAX_REFERENCE_HOPS;
        let mut current_element;
        let mut visited = HashSet::new();

        while hops_left > 0 {
            if visited.contains(&path) {
                return Err(Error::CyclicReference).wrap_with_cost(cost);
            }
            if let Some((key, path_slice)) = path.split_last() {
                current_element = cost_return_on_error!(
                    &mut cost,
                    self.get_raw(path_slice.iter().map(|x| x.as_slice()), key, transaction)
                        .map_err(|e| match e {
                            Error::PathParentLayerNotFound(p) => {
                                Error::CorruptedReferencePathParentLayerNotFound(p)
                            }
                            Error::PathKeyNotFound(p) => {
                                Error::CorruptedReferencePathKeyNotFound(p)
                            }
                            Error::PathNotFound(p) => {
                                Error::CorruptedReferencePathNotFound(p)
                            }
                            _ => e,
                        })
                )
            } else {
                return Err(Error::CorruptedPath("empty path")).wrap_with_cost(cost);
            }
            visited.insert(path.clone());
            match current_element {
                Element::Reference(reference_path, ..) => {
                    path = cost_return_on_error!(
                        &mut cost,
                        path_from_reference_qualified_path_type(reference_path, &path)
                            .wrap_with_cost(OperationCost::default())
                    )
                }
                other => return Ok(other).wrap_with_cost(cost),
            }
            hops_left -= 1;
        }
        Err(Error::ReferenceLimit).wrap_with_cost(cost)
    }

    /// Get tree item without following references
    pub fn get_raw<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        transaction: TransactionArg,
    ) -> CostResult<Element, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: ExactSizeIterator + DoubleEndedIterator + Clone,
    {
        if let Some(transaction) = transaction {
            self.get_raw_on_transaction(path, key, transaction)
        } else {
            self.get_raw_without_transaction(path, key)
        }
    }

    /// Get tree item without following references
    pub fn get_raw_on_transaction<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        transaction: &Transaction,
    ) -> CostResult<Element, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: ExactSizeIterator + DoubleEndedIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let merk_to_get_from: Merk<PrefixedRocksDbTransactionContext> = cost_return_on_error!(
            &mut cost,
            self.open_transactional_merk_at_path(path.into_iter(), transaction)
                .map_err(|e| match e {
                    Error::InvalidParentLayerPath(s) => {
                        Error::PathParentLayerNotFound(s)
                    }
                    _ => e,
                })
        );

        Element::get(&merk_to_get_from, key).add_cost(cost)
    }

    /// Get tree item without following references
    pub fn get_raw_without_transaction<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
    ) -> CostResult<Element, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: ExactSizeIterator + DoubleEndedIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let merk_to_get_from: Merk<PrefixedRocksDbStorageContext> = cost_return_on_error!(
            &mut cost,
            self.open_non_transactional_merk_at_path(path.into_iter())
                .map_err(|e| match e {
                    Error::InvalidParentLayerPath(s) => {
                        Error::PathParentLayerNotFound(s)
                    }
                    _ => e,
                })
        );

        Element::get(&merk_to_get_from, key).add_cost(cost)
    }

    /// Does tree element exist without following references
    pub fn has_raw<'p, P>(
        &self,
        path: P,
        key: &'p [u8],
        transaction: TransactionArg,
    ) -> CostResult<bool, Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: ExactSizeIterator + DoubleEndedIterator + Clone,
    {
        let path_iter = path.into_iter();

        // Merk's items should be written into data storage and checked accordingly
        storage_context_optional_tx!(self.db, path_iter, transaction, storage, {
            storage.flat_map(|s| s.get(key).map_err(|e| e.into()).map_ok(|x| x.is_some()))
        })
    }

    fn check_subtree_exists<'p, P>(
        &self,
        path: P,
        transaction: TransactionArg,
        error: Error,
    ) -> CostResult<(), Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let mut cost = OperationCost::default();

        let path_iter = path.into_iter();
        if path_iter.len() == 0 {
            return Ok(()).wrap_with_cost(cost);
        }

        let mut parent_iter = path_iter;
        let parent_key = parent_iter.next_back().expect("path is not empty");
        let element = if let Some(transaction) = transaction {
            let merk_to_get_from: Merk<PrefixedRocksDbTransactionContext> = cost_return_on_error!(
                &mut cost,
                self.open_transactional_merk_at_path(parent_iter, transaction)
            );

            Element::get(&merk_to_get_from, parent_key)
        } else {
            let merk_to_get_from: Merk<PrefixedRocksDbStorageContext> = cost_return_on_error!(
                &mut cost,
                self.open_non_transactional_merk_at_path(parent_iter)
            );

            Element::get(&merk_to_get_from, parent_key)
        }
        .unwrap_add_cost(&mut cost);
        match element {
            Ok(Element::Tree(..)) | Ok(Element::SumTree(..)) => Ok(()).wrap_with_cost(cost),
            Ok(_) | Err(Error::PathKeyNotFound(_)) => Err(error).wrap_with_cost(cost),
            Err(e) => Err(e).wrap_with_cost(cost),
        }
    }

    pub fn check_subtree_exists_path_not_found<'p, P>(
        &self,
        path: P,
        transaction: TransactionArg,
    ) -> CostResult<(), Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        let path_iter = path.into_iter();
        self.check_subtree_exists(
            path_iter.clone(),
            transaction,
            Error::PathNotFound(format!(
                "subtree doesn't exist at path {:?}",
                path_iter.map(hex::encode).collect::<Vec<String>>()
            )),
        )
    }

    pub fn check_subtree_exists_invalid_path<'p, P>(
        &self,
        path: P,
        transaction: TransactionArg,
    ) -> CostResult<(), Error>
    where
        P: IntoIterator<Item = &'p [u8]>,
        <P as IntoIterator>::IntoIter: DoubleEndedIterator + ExactSizeIterator + Clone,
    {
        self.check_subtree_exists(
            path,
            transaction,
            Error::InvalidPath("subtree doesn't exist".to_owned()),
        )
    }
}