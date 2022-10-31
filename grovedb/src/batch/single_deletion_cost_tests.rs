#[cfg(test)]
mod tests {
    use std::option::Option::None;

    use costs::{
        storage_cost::{
            removal::{
                StorageRemovedBytes,
                StorageRemovedBytes::{
                    BasicStorageRemoval, NoStorageRemoval, SectionedStorageRemoval,
                },
            },
            transition::OperationStorageTransitionType,
            StorageCost,
        },
        OperationCost,
    };
    use integer_encoding::VarInt;
    use intmap::IntMap;

    use crate::{
        batch::GroveDbOp,
        operations::delete::DeleteOptions,
        reference_path::ReferencePathType,
        tests::{make_empty_grovedb, make_test_grovedb, ANOTHER_TEST_LEAF, TEST_LEAF},
        Element, PathQuery,
    };

    #[test]
    fn test_batch_one_deletion_tree_costs_match_non_batch_on_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(vec![], b"key1", Element::empty_tree(), None, None)
            .cost_as_result()
            .expect("expected to insert successfully");

        let tx = db.start_transaction();

        let non_batch_cost = db
            .delete(vec![], b"key1", None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 37
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for empty tree value
        // 32 for node hash
        // 0 for value hash
        // 2 byte for the value_size (required space for 98 + up to 256 for child key)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 37 + 39 = 113

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        tx.rollback().expect("expected to rollback");
        let ops = vec![GroveDbOp::delete_tree_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_item_costs_match_non_batch_on_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::new_item(b"cat".to_vec()),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let tx = db.start_transaction();

        let non_batch_cost = db
            .delete(vec![], b"key1", None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 71
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for required space for bytes
        //   3 bytes for value
        // 32 for node hash
        // 32 for value hash
        // 1 byte for the value_size (required space for 70)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 71 + 39 = 147

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        tx.rollback().expect("expected to rollback");
        let ops = vec![GroveDbOp::delete_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_tree_costs_match_non_batch_without_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(vec![], b"key1", Element::empty_tree(), None, None)
            .cost_as_result()
            .expect("expected to insert successfully");

        let non_batch_cost = db
            .delete(vec![], b"key1", None, None)
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 37
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for empty tree value
        // 32 for node hash
        // 0 for value hash
        // 2 byte for the value_size (required space for 98 + up to 256 for child key)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 37 + 39 = 113

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(vec![], b"key1", Element::empty_tree(), None, None)
            .cost_as_result()
            .expect("expected to insert successfully");

        let ops = vec![GroveDbOp::delete_tree_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, None)
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_item_costs_match_non_batch_without_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::new_item(b"cat".to_vec()),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let non_batch_cost = db
            .delete(vec![], b"key1", None, None)
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 71
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for required space for bytes
        //   3 bytes for value
        // 32 for node hash
        // 32 for value hash
        // 1 byte for the value_size (required space for 70)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 71 + 39 = 147

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::new_item(b"cat".to_vec()),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let ops = vec![GroveDbOp::delete_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, None)
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_tree_with_flags_costs_match_non_batch_on_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::empty_tree_with_flags(Some(b"dog".to_vec())),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let tx = db.start_transaction();

        let non_batch_cost = db
            .delete(vec![], b"key1", None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 116 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 41
        //   1 for the flag option (but no flags)
        //   1 for the flags size
        //   3 bytes for flags
        //   1 for the enum type
        //   1 for empty tree value
        // 32 for node hash
        // 0 for value hash
        // 2 byte for the value_size (required space for 98 + up to 256 for child key)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 37 + 39 = 117

        assert_eq!(insertion_cost.storage_cost.added_bytes, 117);
        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        tx.rollback().expect("expected to rollback");
        let ops = vec![GroveDbOp::delete_tree_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_item_with_flags_costs_match_non_batch_on_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::new_item_with_flags(b"cat".to_vec(), Some(b"apple".to_vec())),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let tx = db.start_transaction();

        let non_batch_cost = db
            .delete(vec![], b"key1", None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 71
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for required space for bytes
        //   3 bytes for value
        // 32 for node hash
        // 32 for value hash
        // 1 byte for the value_size (required space for 70)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 71 + 39 = 147

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        tx.rollback().expect("expected to rollback");
        let ops = vec![GroveDbOp::delete_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, Some(&tx))
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_tree_with_flags_costs_match_non_batch_without_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::empty_tree_with_flags(Some(b"dog".to_vec())),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let non_batch_cost = db
            .delete(vec![], b"key1", None, None)
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 37
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for empty tree value
        // 32 for node hash
        // 0 for value hash
        // 2 byte for the value_size (required space for 98 + up to 256 for child key)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 37 + 39 = 113

        assert_eq!(insertion_cost.storage_cost.added_bytes, 117);

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(vec![], b"key1", Element::empty_tree(), None, None)
            .cost_as_result()
            .expect("expected to insert successfully");

        let ops = vec![GroveDbOp::delete_tree_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, None)
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }

    #[test]
    fn test_batch_one_deletion_item_with_flags_costs_match_non_batch_without_transaction() {
        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::new_item_with_flags(b"cat".to_vec(), Some(b"apple".to_vec())),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let non_batch_cost = db
            .delete(vec![], b"key1", None, None)
            .cost_as_result()
            .expect("expected to delete successfully");

        // Explanation for 113 storage_written_bytes

        // Key -> 37 bytes
        // 32 bytes for the key prefix
        // 4 bytes for the key
        // 1 byte for key_size (required space for 36)

        // Value -> 71
        //   1 for the flag option (but no flags)
        //   1 for the enum type
        //   1 for required space for bytes
        //   3 bytes for value
        // 32 for node hash
        // 32 for value hash
        // 1 byte for the value_size (required space for 70)

        // Parent Hook -> 39
        // Key Bytes 4
        // Hash Size 32
        // Key Length 1
        // Child Heights 2

        // Total 37 + 71 + 39 = 147

        assert_eq!(
            insertion_cost.storage_cost.added_bytes,
            non_batch_cost
                .storage_cost
                .removed_bytes
                .total_removed_bytes()
        );

        let db = make_empty_grovedb();

        let insertion_cost = db
            .insert(
                vec![],
                b"key1",
                Element::new_item_with_flags(b"cat".to_vec(), Some(b"apple".to_vec())),
                None,
                None,
            )
            .cost_as_result()
            .expect("expected to insert successfully");

        let ops = vec![GroveDbOp::delete_run_op(vec![], b"key1".to_vec())];
        let batch_cost = db
            .apply_batch(ops, None, None)
            .cost_as_result()
            .expect("expected to delete successfully");
        assert_eq!(non_batch_cost.storage_cost, batch_cost.storage_cost);
    }
}
