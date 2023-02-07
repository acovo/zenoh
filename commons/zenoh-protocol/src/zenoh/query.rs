//
// Copyright (c) 2022 ZettaScale Technology
//
// This program and the accompanying materials are made available under the
// terms of the Eclipse Public License 2.0 which is available at
// http://www.eclipse.org/legal/epl-2.0, or the Apache License, Version 2.0
// which is available at https://www.apache.org/licenses/LICENSE-2.0.
//
// SPDX-License-Identifier: EPL-2.0 OR Apache-2.0
//
// Contributors:
//   ZettaScale Zenoh Team, <zenoh@zettascale.tech>
//
use crate::{
    core::{WireExpr, ZInt},
    zenoh::DataInfo,
};
use alloc::string::String;
use zenoh_buffers::ZBuf;

/// The kind of consolidation.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum ConsolidationMode {
    /// No consolidation applied: multiple samples may be received for the same key-timestamp.
    None,
    /// Monotonic consolidation immediately forwards samples, except if one with an equal or more recent timestamp
    /// has already been sent with the same key.
    ///
    /// This optimizes latency while potentially reducing bandwidth.
    ///
    /// Note that this doesn't cause re-ordering, but drops the samples for which a more recent timestamp has already
    /// been observed with the same key.
    Monotonic,
    /// Holds back samples to only send the set of samples that had the highest timestamp for their key.
    Latest,
}

/// The `zenoh::queryable::Queryable`s that should be target of a `zenoh::Session::get()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum QueryTarget {
    BestMatching,
    All,
    AllComplete,
    #[cfg(feature = "complete_n")]
    Complete(ZInt),
}

impl Default for QueryTarget {
    fn default() -> Self {
        QueryTarget::BestMatching
    }
}

/// # QueryBody
///
/// QueryBody data structure is optionally included in Query messages
///
/// ```text
///  7 6 5 4 3 2 1 0
/// +-+-+-+---------+
/// ~    DataInfo   ~
/// +---------------+
/// ~    Payload    ~
/// +---------------+
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct QueryBody {
    pub data_info: DataInfo,
    pub payload: ZBuf,
}

impl QueryBody {
    #[cfg(feature = "test")]
    pub fn rand() -> Self {
        use rand::Rng;

        const MIN: usize = 2;
        const MAX: usize = 16;

        let mut rng = rand::thread_rng();

        let data_info = DataInfo::rand();
        let payload = ZBuf::rand(rng.gen_range(MIN..MAX));

        Self { data_info, payload }
    }
}

/// # Query message
///
/// ```text
///  7 6 5 4 3 2 1 0
/// +-+-+-+-+-+-+-+-+
/// |K|B|T|  QUERY  |
/// +-+-+-+---------+
/// ~    KeyExpr     ~ if K==1 then key_expr has suffix
/// +---------------+
/// ~selector_params~
/// +---------------+
/// ~      qid      ~
/// +---------------+
/// ~     target    ~ if T==1
/// +---------------+
/// ~ consolidation ~
/// +---------------+
/// ~   QueryBody   ~ if B==1
/// +---------------+
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Query {
    pub key: WireExpr<'static>,
    pub parameters: String,
    pub qid: ZInt,
    pub target: Option<QueryTarget>,
    pub consolidation: ConsolidationMode,
    pub body: Option<QueryBody>,
}

impl Query {
    #[cfg(feature = "test")]
    pub fn rand() -> Self {
        use rand::{
            distributions::{Alphanumeric, DistString},
            seq::SliceRandom,
            Rng,
        };

        const MIN: usize = 2;
        const MAX: usize = 16;

        let mut rng = rand::thread_rng();

        let key = WireExpr::rand();

        let parameters = if rng.gen_bool(0.5) {
            let len = rng.gen_range(MIN..MAX);
            Alphanumeric.sample_string(&mut rng, len)
        } else {
            String::new()
        };

        let qid: ZInt = rng.gen();

        let target = if rng.gen_bool(0.5) {
            let t = [
                QueryTarget::All,
                QueryTarget::AllComplete,
                QueryTarget::BestMatching,
                #[cfg(feature = "complete_n")]
                QueryTarget::Complete(rng.gen()),
            ];
            let t = t.choose(&mut rng).unwrap();
            Some(*t)
        } else {
            None
        };
        let consolidation = *[
            ConsolidationMode::Latest,
            ConsolidationMode::Monotonic,
            ConsolidationMode::None,
        ]
        .choose(&mut rng)
        .unwrap();

        let body = if rng.gen_bool(0.5) {
            Some(QueryBody::rand())
        } else {
            None
        };

        Self {
            key,
            parameters,
            qid,
            target,
            consolidation,
            body,
        }
    }
}