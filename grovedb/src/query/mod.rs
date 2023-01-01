use itertools::Itertools;
#[cfg(any(feature = "full", feature = "verify"))]
use merk::proofs::query::query_item::QueryItem;
use merk::proofs::query::SubqueryBranch;
#[cfg(any(feature = "full", feature = "verify"))]
use merk::proofs::Query;

#[cfg(any(feature = "full", feature = "verify"))]
use crate::query_result_type::PathKey;
#[cfg(any(feature = "full", feature = "verify"))]
use crate::Error;

#[cfg(any(feature = "full", feature = "verify"))]
#[derive(Debug, Clone)]
pub struct PathQuery {
    // TODO: Make generic over path type
    pub path: Vec<Vec<u8>>,
    pub query: SizedQuery,
}

#[cfg(any(feature = "full", feature = "verify"))]
#[derive(Debug, Clone)]
pub struct SizedQuery {
    pub query: Query,
    pub limit: Option<u16>,
    pub offset: Option<u16>,
}

#[cfg(any(feature = "full", feature = "verify"))]
impl SizedQuery {
    pub const fn new(query: Query, limit: Option<u16>, offset: Option<u16>) -> Self {
        Self {
            query,
            limit,
            offset,
        }
    }
}

#[cfg(any(feature = "full", feature = "verify"))]
impl PathQuery {
    pub const fn new(path: Vec<Vec<u8>>, query: SizedQuery) -> Self {
        Self { path, query }
    }

    pub const fn new_unsized(path: Vec<Vec<u8>>, query: Query) -> Self {
        let query = SizedQuery::new(query, None, None);
        Self { path, query }
    }

    /// Gets the path of all terminal keys
    pub fn terminal_keys(&self, max_results: usize) -> Result<Vec<PathKey>, Error> {
        let mut result: Vec<(Vec<Vec<u8>>, Vec<u8>)> = vec![];
        self.query
            .query
            .terminal_keys(self.path.clone(), max_results, &mut result)
            .map_err(Error::MerkError)?;
        Ok(result)
    }

    /// Combines multiple path queries into one equivalent path query
    /// Restriction: all path must be unique and non subset path
    /// [a] + [a, b] (invalid [a, b] is an extension of [a])
    /// [a, b] + [a, b]
    ///     valid if they both point queries of the same depth
    ///     invalid if they point to queries of different depth
    /// TODO: Currently not allowing unlimited depth queries when paths are
    /// equal     this is possible, should handle later.
    /// [a] + [b] (valid, unique and non subset)
    pub fn merge(mut path_queries: Vec<&PathQuery>) -> Result<Self, Error> {
        if path_queries.is_empty() {
            return Err(Error::InvalidInput(
                "merge function requires at least 1 path query",
            ));
        }
        if path_queries.len() == 1 {
            return Ok(path_queries.remove(0).clone());
        }

        let (common_path, next_index) = PathQuery::get_common_path(&path_queries);

        let mut queries_for_common_path_this_level: Vec<Query> = vec![];

        let mut queries_for_common_path_sub_level: Vec<SubqueryBranch> = vec![];

        // convert all the paths after the common path to queries
        path_queries.into_iter().try_for_each(|path_query| {
            if path_query.query.offset.is_some() {
                return Err(Error::NotSupported(
                    "can not merge pathqueries with offsets",
                ));
            }
            if path_query.query.limit.is_some() {
                return Err(Error::NotSupported(
                    "can not merge pathqueries with limits, consider setting the limit after the \
                     merge",
                ));
            }
            path_query
                .to_subquery_branch_with_offset_start_index(next_index)
                .map(|unsized_path_query| {
                    if unsized_path_query.subquery_path.is_none() {
                        queries_for_common_path_this_level.push(*unsized_path_query.subquery.unwrap());
                    } else {
                        queries_for_common_path_sub_level.push(unsized_path_query);
                    }
                })
        })?;

        let mut merged_query = Query::merge_multiple(queries_for_common_path_this_level);
        // add conditional subqueries
        for mut sub_path_query in queries_for_common_path_sub_level {
            let SubqueryBranch { subquery_path, subquery } = sub_path_query;
            let mut subquery_path = subquery_path.ok_or(Error::CorruptedCodeExecution("subquery path must exist"))?;
            let key = subquery_path.remove(0); //must exist
            merged_query.items.insert(QueryItem::Key(key.clone()));
            let rest_of_path = if subquery_path.is_empty() { None } else { Some(subquery_path) };
            let subquery_branch = SubqueryBranch {
                subquery_path: rest_of_path,
                subquery,
            };
            merged_query.merge_conditional_boxed_subquery(QueryItem::Key(key), subquery_branch);
        }

        Ok(PathQuery::new_unsized(common_path, merged_query))
    }

    /// Checks if any path query is a subset of another by path
    /// i.e [a,b] in [a,b,c]
    /// Also checks for duplicated paths [a,x] and [a,x]
    fn has_subpaths(path_queries: &[&PathQuery]) -> bool {
        // TODO: Improve this
        // Naive solution n^2 time complexity
        for i in 0..path_queries.len() {
            for j in 0..path_queries.len() {
                if i == j {
                    // don't compare the same path instance
                    continue;
                }
                let path_one = &path_queries[i].path;
                let path_two = &path_queries[j].path;

                let bigger_path;
                let smaller_path;

                let path_one_len = path_one.len();
                let path_two_len = path_two.len();

                if path_one_len == path_two_len {
                    // paths are equal, confirm that queries have no subquery
                    if path_queries[i].query.query.has_subquery()
                        || path_queries[j].query.query.has_subquery()
                    {
                        return true;
                    }
                }

                if path_one_len > path_two_len {
                    bigger_path = path_one;
                    smaller_path = path_two;
                } else {
                    bigger_path = path_two;
                    smaller_path = path_one;
                }

                let mut is_subpath = true;

                // here we basically want to check if one vector is an extension of another
                // that means it contains all elements at the same index
                // what we have to do is check that they match at all points
                for n in 0..smaller_path.len() {
                    if bigger_path[n] != smaller_path[n] {
                        // we have divergence before exhausting the smaller path
                        // not subset
                        is_subpath = false;
                    }
                }

                if is_subpath && path_one_len != path_two_len {
                    return true;
                }
            }
        }

        // didn't find a subpath
        false
    }

    /// Given a set of path queries, this returns an array of path keys that are
    /// common across all the path queries.
    /// Also returns the point at which they stopped being equal.
    fn get_common_path(path_queries: &[&PathQuery]) -> (Vec<Vec<u8>>, usize) {
        let min_path_length = path_queries
            .iter()
            .map(|path_query| path_query.path.len())
            .min()
            .expect("expect path_queries length to be 2 or more");

        let mut common_path = vec![];
        let mut level = 0;

        while level < min_path_length {
            let keys_at_level = path_queries
                .iter()
                .map(|path_query| &path_query.path[level])
                .collect::<Vec<_>>();
            let first_key = keys_at_level[0];

            let keys_are_uniform = keys_at_level.iter().all(|&curr_key| curr_key == first_key);

            if keys_are_uniform {
                common_path.push(first_key.to_vec());
                level += 1;
            } else {
                break;
            }
        }
        (common_path, level)
    }

    /// Given a path and a starting point, a query that is equivalent to the
    /// path is generated example: [a, b, c] =>
    ///     query a
    ///         cond a
    ///             query b
    ///                 cond b
    ///                    query c
    fn to_subquery_branch_with_offset_start_index(
        &self,
        start_index: usize,
    ) -> Result<SubqueryBranch, Error> {
        let path = &self.path;


        if path.len() == start_index {
            Ok(SubqueryBranch {
                subquery_path: None,
                subquery: Some(Box::new(self.query.query.clone())),
            })
        } else if path.len() < start_index {
            Err(Error::CorruptedCodeExecution(
                "invalid start index for path query merge",
            ))
        } else {
            let (_, remainder) = path.split_at(start_index);

            Ok(SubqueryBranch {
                subquery_path: Some(remainder.to_vec()),
                subquery: Some(Box::new(self.query.query.clone())),
            })
        }

    }
}

#[cfg(feature = "full")]
#[cfg(test)]
mod tests {
    use merk::proofs::{query::query_item::QueryItem, Query};

    use crate::{
        tests::{common::compare_result_tuples, make_deep_tree, TEST_LEAF},
        Element, Error, GroveDb, PathQuery,
    };

    #[test]
    fn test_has_subpaths() {
        let path_query_one = PathQuery::new_unsized(vec![b"a".to_vec()], Query::new());
        let path_query_two =
            PathQuery::new_unsized(vec![b"c".to_vec(), b"b".to_vec()], Query::new());
        let has_subpaths = PathQuery::has_subpaths(&[&path_query_one, &path_query_two]);
        assert_eq!(has_subpaths, false);

        let path_query_one = PathQuery::new_unsized(vec![b"a".to_vec()], Query::new());
        let path_query_two =
            PathQuery::new_unsized(vec![b"a".to_vec(), b"b".to_vec()], Query::new());

        let has_subpaths = PathQuery::has_subpaths(&[&path_query_one, &path_query_two]);
        assert_eq!(has_subpaths, true);

        let path_query_one =
            PathQuery::new_unsized(vec![b"a".to_vec(), b"b".to_vec()], Query::new());
        let path_query_two =
            PathQuery::new_unsized(vec![b"a".to_vec(), b"b".to_vec()], Query::new());

        let has_subpaths = PathQuery::has_subpaths(&[&path_query_one, &path_query_two]);

        assert_eq!(has_subpaths, false);

        let path_query_one = PathQuery::new_unsized(
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec(), b"d".to_vec()],
            Query::new(),
        );
        let path_query_two = PathQuery::new_unsized(
            vec![b"a".to_vec(), b"b".to_vec(), b"c".to_vec()],
            Query::new(),
        );
        let has_subpaths = PathQuery::has_subpaths(&[&path_query_one, &path_query_two]);
        assert_eq!(has_subpaths, true);
    }

    #[test]
    fn test_same_path_different_query_merge() {
        let temp_db = make_deep_tree();

        // starting with no subquery, just a single path and a key query
        let mut query_one = Query::new();
        query_one.insert_key(b"key1".to_vec());
        let path_query_one =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_one);

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set_one) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set_one.len(), 1);

        let mut query_two = Query::new();
        query_two.insert_key(b"key2".to_vec());
        let path_query_two =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_two);

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set_two) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set_two.len(), 1);

        let merged_path_query = PathQuery::merge(vec![&path_query_one, &path_query_two])
            .expect("should merge path queries");

        let proof = temp_db.prove_query(&merged_path_query).unwrap().unwrap();
        let (_, result_set_tree) = GroveDb::verify_query(proof.as_slice(), &merged_path_query)
            .expect("should execute proof");
        assert_eq!(result_set_tree.len(), 2);
    }

    #[test]
    fn test_different_same_length_path_with_different_query_merge() {
        // Tests for
        // [a, c, Q]
        // [a, m, Q]
        let temp_db = make_deep_tree();

        let mut query_one = Query::new();
        query_one.insert_key(b"key1".to_vec());
        let path_query_one =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_one);

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set_one) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set_one.len(), 1);

        let mut query_two = Query::new();
        query_two.insert_key(b"key4".to_vec());
        let path_query_two =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree4".to_vec()], query_two);

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set_two) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set_two.len(), 1);

        // let merged_path_query = PathQuery::merge(vec![&path_query_one,
        // &path_query_two])     .expect("expect to merge path queries");
        // assert_eq!(merged_path_query.path, vec![TEST_LEAF.to_vec()]);
        // assert_eq!(merged_path_query.query.query.items.len(), 2);
        //
        // let proof = temp_db.prove_query(&merged_path_query).unwrap().unwrap();
        // let (_, result_set_merged) = GroveDb::verify_query(proof.as_slice(),
        // &merged_path_query)     .expect("should execute proof");
        // assert_eq!(result_set_merged.len(), 2);
        //
        // let keys = [b"key1".to_vec(), b"key4".to_vec()];
        // let values = [b"value1".to_vec(), b"value4".to_vec()];
        // let elements = values.map(|x| Element::new_item(x).serialize().unwrap());
        // let expected_result_set: Vec<(Vec<u8>, Vec<u8>)> =
        // keys.into_iter().zip(elements).collect();
        // compare_result_tuples(result_set_merged, expected_result_set);

        // longer length path queries
        let mut query_one = Query::new();
        query_one.insert_all();
        let path_query_one = PathQuery::new_unsized(
            vec![
                b"deep_leaf".to_vec(),
                b"deep_node_1".to_vec(),
                b"deeper_2".to_vec(),
            ],
            query_one.clone(),
        );

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set_one) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set_one.len(), 3);

        let mut query_two = Query::new();
        query_two.insert_all();

        let path_query_two = PathQuery::new_unsized(
            vec![
                b"deep_leaf".to_vec(),
                b"deep_node_2".to_vec(),
                b"deeper_4".to_vec(),
            ],
            query_two.clone(),
        );

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set_two) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set_two.len(), 2);

        let mut query_three = Query::new();
        query_three.insert_range_after(b"key7".to_vec()..);

        let path_query_three = PathQuery::new_unsized(
            vec![
                b"deep_leaf".to_vec(),
                b"deep_node_2".to_vec(),
                b"deeper_3".to_vec(),
            ],
            query_three.clone(),
        );

        let proof = temp_db.prove_query(&path_query_three).unwrap().unwrap();
        let (_, result_set_two) = GroveDb::verify_query(proof.as_slice(), &path_query_three)
            .expect("should execute proof");
        assert_eq!(result_set_two.len(), 2);

        #[rustfmt::skip]
        mod explanation {

    // Tree Structure
    //                                   root
    //              /                      |                       \ (not representing Merk)
    // -----------------------------------------------------------------------------------------
    //         test_leaf            another_test_leaf                deep_leaf
    //       /           \             /         \              /                 \
    // -----------------------------------------------------------------------------------------
    //   innertree     innertree4  innertree2  innertree3  deep_node_1          deep_node_2
    //       |             |           |           |      /          \         /         \
    // -----------------------------------------------------------------------------------------
    //      k2,v2        k4,v4       k3,v3      k4,v4   deeper_1   deeper_2  deeper_3   deeper_4
    //     /     \         |                           |            |         |          |
    //  k1,v1    k3,v3   k5,v5                        /            /          |          |
    // -----------------------------------------------------------------------------------------
    //                                            k2,v2         k5,v5        k8,v8     k10,v10
    //                                           /     \        /    \       /    \       \
    //                                       k1,v1    k3,v3  k4,v4   k6,v6 k7,v7  k9,v9  k11,v11
    //                                                            ↑ (all 3)   ↑     (all 2) ↑
    //                                                      path_query_one    ↑   path_query_two
    //                                                                 path_query_three (2)
    //                                                                   (after 7, so {8,9})

        }

        let merged_path_query =
            PathQuery::merge(vec![&path_query_one, &path_query_two, &path_query_three])
                .expect("expect to merge path queries");
        assert_eq!(merged_path_query.path, vec![b"deep_leaf".to_vec()]);
        assert_eq!(merged_path_query.query.query.items.len(), 2);
        let conditional_subquery_branches = merged_path_query
            .query
            .query
            .conditional_subquery_branches
            .clone()
            .expect("expected to have conditional subquery branches");
        assert_eq!(conditional_subquery_branches.len(), 2);
        let (deep_node_1_query_item, deep_node_1_subquery_branch) =
            conditional_subquery_branches.first().unwrap().clone();
        let (deep_node_2_query_item, deep_node_2_subquery_branch) =
            conditional_subquery_branches.last().unwrap().clone();
        assert_eq!(
            deep_node_1_query_item,
            &QueryItem::Key(b"deep_node_1".to_vec())
        );
        assert_eq!(
            deep_node_2_query_item,
            &QueryItem::Key(b"deep_node_2".to_vec())
        );

        assert_eq!(
            deep_node_1_subquery_branch
                .subquery_path
                .as_ref()
                .expect("expected a subquery_path for deep_node_1"),
            &vec![b"deeper_2".to_vec()]
        );
        assert_eq!(
            *deep_node_1_subquery_branch
                .subquery
                .as_ref()
                .expect("expected a subquery for deep_node_1"),
            Box::new(query_one)
        );

        assert!(
            deep_node_2_subquery_branch.subquery_path.is_none(),
            "there should be no subquery path here"
        );
        let deep_node_2_subquery = deep_node_2_subquery_branch
            .subquery
            .as_ref()
            .expect("expected a subquery for deep_node_2")
            .as_ref();
        let deep_node_2_conditional_subquery_branches = deep_node_2_subquery
            .conditional_subquery_branches
            .as_ref()
            .expect("expected to have conditional subquery branches");
        assert_eq!(deep_node_2_conditional_subquery_branches.len(), 2);

        // deeper 4 was query 2
        let (deeper_4_query_item, deeper_4_subquery_branch) =
            deep_node_2_conditional_subquery_branches
                .first()
                .unwrap()
                .clone();
        let (deeper_3_query_item, deeper_3_subquery_branch) =
            deep_node_2_conditional_subquery_branches
                .last()
                .unwrap()
                .clone();

        assert_eq!(deeper_3_query_item, &QueryItem::Key(b"deeper_3".to_vec()));
        assert_eq!(deeper_4_query_item, &QueryItem::Key(b"deeper_4".to_vec()));

        assert!(
            deeper_3_subquery_branch.subquery_path.is_none(),
            "there should be no subquery path here"
        );
        assert_eq!(
            *deeper_3_subquery_branch
                .subquery
                .as_ref()
                .expect("expected a subquery for deeper_3"),
            Box::new(query_three)
        );

        assert!(
            deeper_4_subquery_branch.subquery_path.is_none(),
            "there should be no subquery path here"
        );
        assert_eq!(
            *deeper_4_subquery_branch
                .subquery
                .as_ref()
                .expect("expected a subquery for deeper_4"),
            Box::new(query_two)
        );

        let proof = temp_db.prove_query(&merged_path_query).unwrap().unwrap();
        let (_, result_set_merged) = GroveDb::verify_query(proof.as_slice(), &merged_path_query)
            .expect("should execute proof");
        assert_eq!(result_set_merged.len(), 7);

        let keys = [
            b"key4".to_vec(),
            b"key5".to_vec(),
            b"key6".to_vec(),
            b"key8".to_vec(),
            b"key9".to_vec(),
            b"key10".to_vec(),
            b"key11".to_vec(),
        ];
        let values = [
            b"value4".to_vec(),
            b"value5".to_vec(),
            b"value6".to_vec(),
            b"value8".to_vec(),
            b"value9".to_vec(),
            b"value10".to_vec(),
            b"value11".to_vec(),
        ];
        let elements = values.map(|x| Element::new_item(x).serialize().unwrap());
        let expected_result_set: Vec<(Vec<u8>, Vec<u8>)> = keys.into_iter().zip(elements).collect();
        compare_result_tuples(result_set_merged, expected_result_set);
    }

    #[test]
    fn test_different_length_paths_merge() {
        let temp_db = make_deep_tree();

        let mut query_one = Query::new();
        query_one.insert_all();

        let mut subq = Query::new();
        subq.insert_all();
        query_one.set_subquery(subq);

        let path_query_one = PathQuery::new_unsized(
            vec![b"deep_leaf".to_vec(), b"deep_node_1".to_vec()],
            query_one,
        );

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set_one) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set_one.len(), 6);

        let mut query_two = Query::new();
        query_two.insert_all();

        let path_query_two = PathQuery::new_unsized(
            vec![
                b"deep_leaf".to_vec(),
                b"deep_node_2".to_vec(),
                b"deeper_4".to_vec(),
            ],
            query_two,
        );

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set_two) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set_two.len(), 2);

        let merged_path_query = PathQuery::merge(vec![&path_query_one, &path_query_two])
            .expect("expect to merge path queries");
        assert_eq!(merged_path_query.path, vec![b"deep_leaf".to_vec()]);

        let proof = temp_db.prove_query(&merged_path_query).unwrap().unwrap();
        let (_, result_set_merged) = GroveDb::verify_query(proof.as_slice(), &merged_path_query)
            .expect("should execute proof");
        assert_eq!(result_set_merged.len(), 8);

        let keys = [
            b"key1".to_vec(),
            b"key2".to_vec(),
            b"key3".to_vec(),
            b"key4".to_vec(),
            b"key5".to_vec(),
            b"key6".to_vec(),
            b"key10".to_vec(),
            b"key11".to_vec(),
        ];
        let values = [
            b"value1".to_vec(),
            b"value2".to_vec(),
            b"value3".to_vec(),
            b"value4".to_vec(),
            b"value5".to_vec(),
            b"value6".to_vec(),
            b"value10".to_vec(),
            b"value11".to_vec(),
        ];
        let elements = values.map(|x| Element::new_item(x).serialize().unwrap());
        let expected_result_set: Vec<(Vec<u8>, Vec<u8>)> = keys.into_iter().zip(elements).collect();
        compare_result_tuples(result_set_merged, expected_result_set);
    }

    #[test]
    fn test_same_path_and_different_path_query_merge() {
        let temp_db = make_deep_tree();

        let mut query_one = Query::new();
        query_one.insert_key(b"key1".to_vec());
        let path_query_one =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_one);

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set.len(), 1);

        let mut query_two = Query::new();
        query_two.insert_key(b"key2".to_vec());
        let path_query_two =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_two);

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set.len(), 1);

        let mut query_three = Query::new();
        query_three.insert_all();
        let path_query_three = PathQuery::new_unsized(
            vec![TEST_LEAF.to_vec(), b"innertree4".to_vec()],
            query_three,
        );

        let proof = temp_db.prove_query(&path_query_three).unwrap().unwrap();
        let (_, result_set) = GroveDb::verify_query(proof.as_slice(), &path_query_three)
            .expect("should execute proof");
        assert_eq!(result_set.len(), 2);

        let merged_path_query =
            PathQuery::merge(vec![&path_query_one, &path_query_two, &path_query_three])
                .expect("should merge three queries");

        let proof = temp_db.prove_query(&merged_path_query).unwrap().unwrap();
        let (_, result_set) = GroveDb::verify_query(proof.as_slice(), &merged_path_query)
            .expect("should execute proof");
        assert_eq!(result_set.len(), 4);
    }

    #[test]
    fn test_equal_path_merge() {
        // [a, b, Q]
        // [a, b, Q2]
        // We should be able to merge this if Q and Q2 have no subqueries.

        let temp_db = make_deep_tree();

        let mut query_one = Query::new();
        query_one.insert_key(b"key1".to_vec());
        let path_query_one =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_one);

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set.len(), 1);

        let mut query_two = Query::new();
        query_two.insert_key(b"key2".to_vec());
        let path_query_two =
            PathQuery::new_unsized(vec![TEST_LEAF.to_vec(), b"innertree".to_vec()], query_two);

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set.len(), 1);

        let merged_path_query = PathQuery::merge(vec![&path_query_one, &path_query_two])
            .expect("should merge three queries");

        let proof = temp_db.prove_query(&merged_path_query).unwrap().unwrap();
        let (_, result_set) = GroveDb::verify_query(proof.as_slice(), &merged_path_query)
            .expect("should execute proof");
        assert_eq!(result_set.len(), 2);

        // [a, b, Q]
        // [a, b, c, Q2] (rolled up to) [a, b, Q3] where Q3 combines [c, Q2]
        // this should fail as [a, b] is a subpath of [a, b, c]
        let mut query_one = Query::new();
        query_one.insert_all();
        let path_query_one = PathQuery::new_unsized(
            vec![b"deep_leaf".to_vec(), b"deep_node_1".to_vec()],
            query_one,
        );

        let proof = temp_db.prove_query(&path_query_one).unwrap().unwrap();
        let (_, result_set) =
            GroveDb::verify_query(proof.as_slice(), &path_query_one).expect("should execute proof");
        assert_eq!(result_set.len(), 2);

        let mut query_one = Query::new();
        query_one.insert_key(b"deeper_1".to_vec());

        let mut subq = Query::new();
        subq.insert_all();
        query_one.set_subquery(subq);

        let path_query_two = PathQuery::new_unsized(
            vec![b"deep_leaf".to_vec(), b"deep_node_1".to_vec()],
            query_one,
        );

        let proof = temp_db.prove_query(&path_query_two).unwrap().unwrap();
        let (_, result_set) =
            GroveDb::verify_query(proof.as_slice(), &path_query_two).expect("should execute proof");
        assert_eq!(result_set.len(), 3);

        let merged_path_query = PathQuery::merge(vec![&path_query_one, &path_query_two]);

        assert!(matches!(
            merged_path_query,
            Err(Error::InvalidInput(
                "path query path's should be non subset"
            ))
        ));
    }
}
