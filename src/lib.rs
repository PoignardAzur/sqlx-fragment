// Copyright 2024 Olivier FAURE
// SPDX-License-Identifier: Apache-2.0 OR MIT

//! API for pushing formatted chunks of SQL to an SQLX QueryBuilder

mod builder2;

use sqlx::{Database, QueryBuilder};

fn push_fragment<'args, DB: Database>(
    query: &mut QueryBuilder<'args, DB>,
    fragment: QueryBuilder<'args, DB>,
) {
    //let fragment = fragment.build();
    //fragment.take_arguments().unwrap();

    //query.push_sql(fragment);
}
