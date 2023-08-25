// MIT LICENSE
//
// Copyright (c) 2021 Dash Core Group
//
// Permission is hereby granted, free of charge, to any
// person obtaining a copy of this software and associated
// documentation files (the "Software"), to deal in the
// Software without restriction, including without
// limitation the rights to use, copy, modify, merge,
// publish, distribute, sublicense, and/or sell copies of
// the Software, and to permit persons to whom the Software
// is furnished to do so, subject to the following
// conditions:
//
// The above copyright notice and this permission notice
// shall be included in all copies or substantial portions
// of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
// ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
// TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
// PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
// SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
// CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
// OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
// IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
// DEALINGS IN THE SOFTWARE.

//! Chunk proofs

#[cfg(feature = "full")]
use grovedb_costs::{
    cost_return_on_error, cost_return_on_error_no_add, CostResult, CostsExt, OperationCost,
};
#[cfg(feature = "full")]
use grovedb_storage::RawIterator;
#[cfg(feature = "full")]
use {
    super::tree::{execute, Tree as ProofTree},
    crate::tree::CryptoHash,
    crate::tree::Tree,
};

#[cfg(feature = "full")]
use super::{Node, Op};
#[cfg(feature = "full")]
use crate::{
    error::Error,
    tree::{Fetch, RefWalker},
    Error::EdError,
    TreeFeatureType::BasicMerk,
};

mod binary_range;
#[cfg(feature = "full")]
// TODO: remove from here
pub mod chunk2;
#[cfg(feature = "full")]
// TODO: remove from here
pub mod util;
// TODO: remove from here
pub mod error;
// TODO: remove from here
pub mod chunk_op;

/// The minimum number of layers the trunk will be guaranteed to have before
/// splitting into multiple chunks. If the tree's height is less than double
/// this value, the trunk should be verified as a leaf chunk.
#[cfg(feature = "full")]
pub const MIN_TRUNK_HEIGHT: usize = 5;

#[cfg(feature = "full")]
impl<'a, S> RefWalker<'a, S>
where
    S: Fetch + Sized + Clone,
{
    /// Generates a trunk proof by traversing the tree.
    ///
    /// Returns a tuple containing the produced proof, and a boolean indicating
    /// whether or not there will be more chunks to follow. If the chunk
    /// contains the entire tree, the boolean will be `false`, if the chunk
    /// is abridged and will be connected to leaf chunks, it will be `true`.
    pub fn create_trunk_proof(&mut self) -> CostResult<(Vec<Op>, bool), Error> {
        let approx_size = 2usize.pow((self.tree().height() / 2) as u32) * 3;
        let mut proof = Vec::with_capacity(approx_size);

        self.traverse_for_height_proof(&mut proof, 1)
            .flat_map_ok(|trunk_height| {
                if trunk_height < MIN_TRUNK_HEIGHT {
                    proof.clear();
                    self.traverse_for_trunk(&mut proof, usize::MAX, true)
                        .map_ok(|_| Ok((proof, false)))
                } else {
                    self.traverse_for_trunk(&mut proof, trunk_height, true)
                        .map_ok(|_| Ok((proof, true)))
                }
            })
            .flatten()
    }

    /// Traverses down the left edge of the tree and pushes ops to the proof, to
    /// act as a proof of the height of the tree. This is the first step in
    /// generating a trunk proof.
    fn traverse_for_height_proof(
        &mut self,
        proof: &mut Vec<Op>,
        depth: usize,
    ) -> CostResult<usize, Error> {
        let mut cost = OperationCost::default();
        let maybe_left = match self.walk(true).unwrap_add_cost(&mut cost) {
            Ok(maybe_left) => maybe_left,
            Err(e) => {
                return Err(e).wrap_with_cost(cost);
            }
        };
        let has_left_child = maybe_left.is_some();

        let trunk_height = if let Some(mut left) = maybe_left {
            match left
                .traverse_for_height_proof(proof, depth + 1)
                .unwrap_add_cost(&mut cost)
            {
                Ok(x) => x,
                Err(e) => return Err(e).wrap_with_cost(cost),
            }
        } else {
            depth / 2
        };

        if depth > trunk_height {
            proof.push(Op::Push(self.to_kvhash_node()));

            if has_left_child {
                proof.push(Op::Parent);
            }

            if let Some(right) = self.tree().link(false) {
                proof.push(Op::Push(Node::Hash(*right.hash())));
                proof.push(Op::Child);
            }
        }

        Ok(trunk_height).wrap_with_cost(cost)
    }

    /// Traverses down the tree and adds KV push ops for all nodes up to a
    /// certain depth. This expects the proof to contain a height proof as
    /// generated by `traverse_for_height_proof`.
    fn traverse_for_trunk(
        &mut self,
        proof: &mut Vec<Op>,
        remaining_depth: usize,
        is_leftmost: bool,
    ) -> CostResult<(), Error> {
        let mut cost = OperationCost::default();

        if remaining_depth == 0 {
            // return early if we have reached bottom of trunk

            // for leftmost node, we already have height proof
            if is_leftmost {
                return Ok(()).wrap_with_cost(cost);
            }

            // add this node's hash
            proof.push(Op::Push(self.to_hash_node().unwrap_add_cost(&mut cost)));

            return Ok(()).wrap_with_cost(cost);
        }

        // traverse left
        let has_left_child = self.tree().link(true).is_some();
        if has_left_child {
            let mut left = cost_return_on_error!(&mut cost, self.walk(true)).unwrap();
            cost_return_on_error!(
                &mut cost,
                left.traverse_for_trunk(proof, remaining_depth - 1, is_leftmost)
            );
        }

        // add this node's data
        proof.push(Op::Push(self.to_kv_value_hash_feature_type_node()));

        if has_left_child {
            proof.push(Op::Parent);
        }

        // traverse right
        if let Some(mut right) = cost_return_on_error!(&mut cost, self.walk(false)) {
            cost_return_on_error!(
                &mut cost,
                right.traverse_for_trunk(proof, remaining_depth - 1, false)
            );
            proof.push(Op::Child);
        }

        Ok(()).wrap_with_cost(cost)
    }
}

/// Builds a chunk proof by iterating over values in a RocksDB, ending the chunk
/// when a node with key `end_key` is encountered.
///
/// Advances the iterator for all nodes in the chunk and the `end_key` (if any).
#[cfg(feature = "full")]
pub(crate) fn get_next_chunk(
    iter: &mut impl RawIterator,
    end_key: Option<&[u8]>,
) -> CostResult<Vec<Op>, Error> {
    let mut cost = OperationCost::default();

    let mut chunk = Vec::with_capacity(512);
    let mut stack = Vec::with_capacity(32);
    let mut node = Tree::new(vec![], vec![], None, BasicMerk).unwrap_add_cost(&mut cost);

    while iter.valid().unwrap_add_cost(&mut cost) {
        let key = iter.key().unwrap_add_cost(&mut cost).unwrap();

        if let Some(end_key) = end_key {
            if key == end_key {
                break;
            }
        }

        let encoded_node = iter.value().unwrap_add_cost(&mut cost).unwrap();
        cost_return_on_error_no_add!(
            &cost,
            Tree::decode_into(&mut node, vec![], encoded_node).map_err(EdError)
        );

        // TODO: Only use the KVValueHash if needed, saves 32 bytes
        //  only needed when dealing with references and trees
        let kv = Node::KVValueHashFeatureType(
            key.to_vec(),
            node.value_ref().to_vec(),
            *node.value_hash(),
            node.feature_type(),
        );

        chunk.push(Op::Push(kv));

        if node.link(true).is_some() {
            chunk.push(Op::Parent);
        }

        if let Some(child) = node.link(false) {
            stack.push(child.key().to_vec());
        } else {
            while let Some(top_key) = stack.last() {
                if key < top_key.as_slice() {
                    break;
                }
                stack.pop();
                chunk.push(Op::Child);
            }
        }

        iter.next().unwrap_add_cost(&mut cost);
    }

    if iter.valid().unwrap_add_cost(&mut cost) {
        iter.next().unwrap_add_cost(&mut cost);
    }

    Ok(chunk).wrap_with_cost(cost)
}

/// Verifies a leaf chunk proof by executing its operators. Checks that there
/// were no abridged nodes (Hash or KVHash) and the proof hashes to
/// `expected_hash`.
#[cfg(feature = "full")]
#[allow(dead_code)] // TODO: remove when proofs will be enabled
pub(crate) fn verify_leaf<I: Iterator<Item = Result<Op, Error>>>(
    ops: I,
    expected_hash: CryptoHash,
) -> CostResult<ProofTree, Error> {
    execute(ops, false, |node| match node {
        Node::KVValueHash(..) | Node::KV(..) | Node::KVValueHashFeatureType(..) => Ok(()),
        _ => Err(Error::OldChunkRestoringError(
            "Leaf chunks must contain full subtree".to_string(),
        )),
    })
    .flat_map_ok(|tree| {
        tree.hash().map(|hash| {
            if hash != expected_hash {
                Error::OldChunkRestoringError(format!(
                    "Leaf chunk proof did not match expected hash\n\tExpected: {:?}\n\tActual: \
                     {:?}",
                    expected_hash,
                    tree.hash()
                ));
            }
            Ok(tree)
        })
    })
}

/// Verifies a trunk chunk proof by executing its operators. Ensures the
/// resulting tree contains a valid height proof, the trunk is the correct
/// height, and all of its inner nodes are not abridged. Returns the tree and
/// the height given by the height proof.
#[cfg(feature = "full")]
pub(crate) fn verify_trunk<I: Iterator<Item = Result<Op, Error>>>(
    ops: I,
) -> CostResult<(ProofTree, usize), Error> {
    let mut cost = OperationCost::default();

    fn verify_height_proof(tree: &ProofTree) -> Result<usize, Error> {
        Ok(match tree.child(true) {
            Some(child) => {
                if let Node::Hash(_) = child.tree.node {
                    return Err(Error::OldChunkRestoringError(
                        "Expected height proof to only contain KV and KVHash nodes".to_string(),
                    ));
                }
                verify_height_proof(&child.tree)? + 1
            }
            None => 1,
        })
    }

    fn verify_completeness(
        tree: &ProofTree,
        remaining_depth: usize,
        leftmost: bool,
    ) -> Result<(), Error> {
        let recurse = |left, leftmost| {
            if let Some(child) = tree.child(left) {
                verify_completeness(&child.tree, remaining_depth - 1, left && leftmost)?;
            }
            Ok(())
        };

        if remaining_depth > 0 {
            match tree.node {
                Node::KVValueHash(..) | Node::KV(..) | Node::KVValueHashFeatureType(..) => {}
                _ => {
                    return Err(Error::OldChunkRestoringError(
                        "Expected trunk inner nodes to contain keys and values".to_string(),
                    ))
                }
            }
            recurse(true, leftmost)?;
            recurse(false, false)
        } else if !leftmost {
            match tree.node {
                Node::Hash(_) => Ok(()),
                _ => Err(Error::OldChunkRestoringError(
                    "Expected trunk leaves to contain Hash nodes".to_string(),
                )),
            }
        } else {
            match &tree.node {
                Node::KVHash(_) => Ok(()),
                _ => Err(Error::OldChunkRestoringError(
                    "Expected leftmost trunk leaf to contain KVHash node".to_string(),
                )),
            }
        }
    }

    let mut kv_only = true;
    let tree = cost_return_on_error!(
        &mut cost,
        execute(ops, false, |node| {
            kv_only &= matches!(node, Node::KVValueHash(..))
                || matches!(node, Node::KV(..))
                || matches!(node, Node::KVValueHashFeatureType(..));
            Ok(())
        })
    );

    let height = cost_return_on_error_no_add!(&cost, verify_height_proof(&tree));
    let trunk_height = height / 2;

    if trunk_height < MIN_TRUNK_HEIGHT {
        if !kv_only {
            return Err(Error::OldChunkRestoringError(
                "Leaf chunks must contain full subtree".to_string(),
            ))
            .wrap_with_cost(cost);
        }
    } else {
        cost_return_on_error_no_add!(&cost, verify_completeness(&tree, trunk_height, true));
    }

    Ok((tree, height)).wrap_with_cost(cost)
}

#[cfg(feature = "full")]
#[cfg(test)]
mod tests {
    use std::usize;

    use grovedb_costs::storage_cost::removal::StorageRemovedBytes::NoStorageRemoval;
    use grovedb_storage::StorageContext;

    use super::{super::tree::Tree, *};
    use crate::{
        test_utils::*,
        tree::{NoopCommit, PanicSource, Tree as BaseTree},
    };

    #[derive(Default)]
    struct NodeCounts {
        hash: usize,
        kv_hash: usize,
        kv: usize,
        kv_value_hash: usize,
        kv_digest: usize,
        kv_ref_value_hash: usize,
        kv_value_hash_feature_type: usize,
    }

    fn count_node_types(tree: Tree) -> NodeCounts {
        let mut counts = NodeCounts::default();

        tree.visit_nodes(&mut |node| {
            match node {
                Node::Hash(_) => counts.hash += 1,
                Node::KVHash(_) => counts.kv_hash += 1,
                Node::KV(..) => counts.kv += 1,
                Node::KVValueHash(..) => counts.kv_value_hash += 1,
                Node::KVDigest(..) => counts.kv_digest += 1,
                Node::KVRefValueHash(..) => counts.kv_ref_value_hash += 1,
                Node::KVValueHashFeatureType(..) => counts.kv_value_hash_feature_type += 1,
            };
        });

        counts
    }

    #[test]
    fn small_trunk_roundtrip() {
        let mut tree = make_tree_seq(31);
        let mut walker = RefWalker::new(&mut tree, PanicSource {});

        let (proof, has_more) = walker.create_trunk_proof().unwrap().unwrap();
        assert!(!has_more);

        // println!("{:?}", &proof);
        let (trunk, _) = verify_trunk(proof.into_iter().map(Ok)).unwrap().unwrap();

        let counts = count_node_types(trunk);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_value_hash_feature_type, 32);
        assert_eq!(counts.kv_hash, 0);
    }

    #[test]
    fn big_trunk_roundtrip() {
        let mut tree = make_tree_seq(2u64.pow(MIN_TRUNK_HEIGHT as u32 * 2 + 1) - 1);
        let mut walker = RefWalker::new(&mut tree, PanicSource {});

        let (proof, has_more) = walker.create_trunk_proof().unwrap().unwrap();
        assert!(has_more);
        let (trunk, _) = verify_trunk(proof.into_iter().map(Ok)).unwrap().unwrap();

        let counts = count_node_types(trunk);
        // are these formulas correct for all values of `MIN_TRUNK_HEIGHT`? 🤔
        assert_eq!(
            counts.hash,
            2usize.pow(MIN_TRUNK_HEIGHT as u32) + MIN_TRUNK_HEIGHT - 1
        );
        assert_eq!(
            counts.kv_value_hash_feature_type,
            2usize.pow(MIN_TRUNK_HEIGHT as u32) - 1
        );
        assert_eq!(counts.kv_hash, MIN_TRUNK_HEIGHT + 1);
    }

    #[test]
    fn one_node_tree_trunk_roundtrip() {
        let mut tree = BaseTree::new(vec![0], vec![], None, BasicMerk).unwrap();
        tree.commit(
            &mut NoopCommit {},
            &|_, _| Ok(0),
            &mut |_, _, _| Ok((false, None)),
            &mut |_, _, _| Ok((NoStorageRemoval, NoStorageRemoval)),
        )
        .unwrap()
        .unwrap();

        let mut walker = RefWalker::new(&mut tree, PanicSource {});
        let (proof, has_more) = walker.create_trunk_proof().unwrap().unwrap();
        assert!(!has_more);

        let (trunk, _) = verify_trunk(proof.into_iter().map(Ok)).unwrap().unwrap();
        let counts = count_node_types(trunk);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_value_hash_feature_type, 1);
        assert_eq!(counts.kv_hash, 0);
    }

    #[test]
    fn two_node_right_heavy_tree_trunk_roundtrip() {
        // 0
        //  \
        //   1
        let mut tree = BaseTree::new(vec![0], vec![], None, BasicMerk)
            .unwrap()
            .attach(
                false,
                Some(BaseTree::new(vec![1], vec![], None, BasicMerk).unwrap()),
            );
        tree.commit(
            &mut NoopCommit {},
            &|_, _| Ok(0),
            &mut |_, _, _| Ok((false, None)),
            &mut |_, _, _| Ok((NoStorageRemoval, NoStorageRemoval)),
        )
        .unwrap()
        .unwrap();
        let mut walker = RefWalker::new(&mut tree, PanicSource {});
        let (proof, has_more) = walker.create_trunk_proof().unwrap().unwrap();
        assert!(!has_more);

        let (trunk, _) = verify_trunk(proof.into_iter().map(Ok)).unwrap().unwrap();
        let counts = count_node_types(trunk);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_value_hash_feature_type, 2);
        assert_eq!(counts.kv_hash, 0);
    }

    #[test]
    fn two_node_left_heavy_tree_trunk_roundtrip() {
        //   1
        //  /
        // 0
        let mut tree = BaseTree::new(vec![1], vec![], None, BasicMerk)
            .unwrap()
            .attach(
                true,
                Some(BaseTree::new(vec![0], vec![], None, BasicMerk).unwrap()),
            );
        tree.commit(
            &mut NoopCommit {},
            &|_, _| Ok(0),
            &mut |_, _, _| Ok((false, None)),
            &mut |_, _, _| Ok((NoStorageRemoval, NoStorageRemoval)),
        )
        .unwrap()
        .unwrap();
        let mut walker = RefWalker::new(&mut tree, PanicSource {});
        let (proof, has_more) = walker.create_trunk_proof().unwrap().unwrap();
        assert!(!has_more);

        let (trunk, _) = verify_trunk(proof.into_iter().map(Ok)).unwrap().unwrap();
        let counts = count_node_types(trunk);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_value_hash_feature_type, 2);
        assert_eq!(counts.kv_hash, 0);
    }

    #[test]
    fn three_node_tree_trunk_roundtrip() {
        //   1
        //  / \
        // 0   2
        let mut tree = BaseTree::new(vec![1], vec![], None, BasicMerk)
            .unwrap()
            .attach(
                true,
                Some(BaseTree::new(vec![0], vec![], None, BasicMerk).unwrap()),
            )
            .attach(
                false,
                Some(BaseTree::new(vec![2], vec![], None, BasicMerk).unwrap()),
            );
        tree.commit(
            &mut NoopCommit {},
            &|_, _| Ok(0),
            &mut |_, _, _| Ok((false, None)),
            &mut |_, _, _| Ok((NoStorageRemoval, NoStorageRemoval)),
        )
        .unwrap()
        .unwrap();

        let mut walker = RefWalker::new(&mut tree, PanicSource {});
        let (proof, has_more) = walker.create_trunk_proof().unwrap().unwrap();
        assert!(!has_more);

        let (trunk, _) = verify_trunk(proof.into_iter().map(Ok)).unwrap().unwrap();
        let counts = count_node_types(trunk);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_value_hash_feature_type, 3);
        assert_eq!(counts.kv_hash, 0);
    }

    #[test]
    fn leaf_chunk_roundtrip() {
        let mut merk = TempMerk::new();
        let batch = make_batch_seq(0..31);
        merk.apply::<_, Vec<_>>(batch.as_slice(), &[], None)
            .unwrap()
            .unwrap();

        merk.commit();

        let root_node = merk.tree.take();
        let root_key = root_node.as_ref().unwrap().key().to_vec();
        merk.tree.set(root_node);

        // whole tree as 1 leaf
        let mut iter = merk.storage.raw_iter();
        iter.seek_to_first().unwrap();
        let chunk = get_next_chunk(&mut iter, None).unwrap().unwrap();
        let ops = chunk.into_iter().map(Ok);
        let chunk = verify_leaf(ops, merk.root_hash().unwrap())
            .unwrap()
            .unwrap();
        let counts = count_node_types(chunk);
        assert_eq!(counts.kv_value_hash_feature_type, 31);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_hash, 0);
        drop(iter);

        let mut iter = merk.storage.raw_iter();
        iter.seek_to_first().unwrap();

        // left leaf
        let chunk = get_next_chunk(&mut iter, Some(root_key.as_slice()))
            .unwrap()
            .unwrap();
        let ops = chunk.into_iter().map(Ok);
        let chunk = verify_leaf(
            ops,
            [
                78, 230, 25, 188, 163, 2, 169, 185, 254, 174, 196, 206, 162, 187, 245, 188, 74, 70,
                220, 160, 35, 78, 120, 122, 61, 90, 241, 105, 35, 180, 133, 98,
            ],
        )
        .unwrap()
        .unwrap();
        let counts = count_node_types(chunk);
        assert_eq!(counts.kv_value_hash_feature_type, 15);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_hash, 0);

        // right leaf
        let chunk = get_next_chunk(&mut iter, None).unwrap().unwrap();
        let ops = chunk.into_iter().map(Ok);
        let chunk = verify_leaf(
            ops,
            [
                21, 147, 223, 29, 106, 19, 23, 38, 233, 134, 245, 44, 246, 179, 48, 19, 111, 50,
                19, 191, 134, 37, 165, 5, 35, 111, 233, 213, 212, 5, 92, 45,
            ],
        )
        .unwrap()
        .unwrap();
        let counts = count_node_types(chunk);
        assert_eq!(counts.kv_value_hash_feature_type, 15);
        assert_eq!(counts.hash, 0);
        assert_eq!(counts.kv_hash, 0);
    }
}
