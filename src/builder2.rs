//! Runtime query-builder API.

use std::fmt::Display;
use std::fmt::Write;
use std::marker::PhantomData;

use sqlx::database::Database;
use sqlx::encode::Encode;
use sqlx::query::Query;
use sqlx::query::QueryAs;
use sqlx::query::QueryScalar;
use sqlx::types::Type;
use sqlx::Either;
use sqlx::FromRow;
use sqlx::{Arguments, IntoArguments};

/// A builder type for constructing queries at runtime.
///
/// See [`.push_values()`][Self::push_values] for an example of building a bulk `INSERT` statement.
/// Note, however, that with Postgres you can get much better performance by using arrays
/// and `UNNEST()`. [See our FAQ] for details.
///
/// [See our FAQ]: https://github.com/launchbadge/sqlx/blob/master/FAQ.md#how-can-i-bind-an-array-to-a-values-clause-how-can-i-do-bulk-inserts
pub struct QueryBuilder<'args, DB>
where
    DB: Database,
{
    query: String,
    init_len: usize,
    arguments: Option<<DB as Database>::Arguments<'args>>,
}

impl<'args, DB: Database> Default for QueryBuilder<'args, DB> {
    fn default() -> Self {
        QueryBuilder {
            init_len: 0,
            query: String::default(),
            arguments: Some(Default::default()),
        }
    }
}

impl<'args, DB: Database> QueryBuilder<'args, DB>
where
    DB: Database,
{
    // `init` is provided because a query will almost always start with a constant fragment
    // such as `INSERT INTO ...` or `SELECT ...`, etc.
    /// Start building a query with an initial SQL fragment, which may be an empty string.
    pub fn new(init: impl Into<String>) -> Self
    where
        <DB as Database>::Arguments<'args>: Default,
    {
        let init = init.into();

        QueryBuilder {
            init_len: init.len(),
            query: init,
            arguments: Some(Default::default()),
        }
    }

    /// Construct a `QueryBuilder` with existing SQL and arguments.
    ///
    /// ### Note
    /// This does *not* check if `arguments` is valid for the given SQL.
    pub fn with_arguments<A>(init: impl Into<String>, arguments: A) -> Self
    where
        DB: Database,
        A: IntoArguments<'args, DB>,
    {
        let init = init.into();

        QueryBuilder {
            init_len: init.len(),
            query: init,
            arguments: Some(arguments.into_arguments()),
        }
    }

    #[inline]
    fn sanity_check(&self) {
        assert!(
            self.arguments.is_some(),
            "QueryBuilder must be reset before reuse after `.build()`"
        );
    }

    /// Append a SQL fragment to the query.
    ///
    /// May be a string or anything that implements `Display`.
    /// You can also use `format_args!()` here to push a formatted string without an intermediate
    /// allocation.
    ///
    /// ### Warning: Beware SQL Injection Vulnerabilities and Untrusted Input!
    /// You should *not* use this to insert input directly into the query from an untrusted user as
    /// this can be used by an attacker to extract sensitive data or take over your database.
    ///
    /// Security breaches due to SQL injection can cost your organization a lot of money from
    /// damage control and lost clients, betray the trust of your users in your system, and are just
    /// plain embarrassing. If you are unfamiliar with the threat that SQL injection imposes, you
    /// should take some time to learn more about it before proceeding:
    ///
    /// * [SQL Injection on OWASP.org](https://owasp.org/www-community/attacks/SQL_Injection)
    /// * [SQL Injection on Wikipedia](https://en.wikipedia.org/wiki/SQL_injection)
    ///     * See "Examples" for notable instances of security breaches due to SQL injection.
    ///
    /// This method does *not* perform sanitization. Instead, you should use
    /// [`.push_bind()`][Self::push_bind] which inserts a placeholder into the query and then
    /// sends the possibly untrustworthy value separately (called a "bind argument") so that it
    /// cannot be misinterpreted by the database server.
    ///
    /// Note that you should still at least have some sort of sanity checks on the values you're
    /// sending as that's just good practice and prevent other types of attacks against your system,
    /// e.g. check that strings aren't too long, numbers are within expected ranges, etc.
    pub fn push(&mut self, sql: impl Display) -> &mut Self {
        self.sanity_check();

        write!(self.query, "{sql}").expect("error formatting `sql`");

        self
    }

    /// Push a bind argument placeholder (`?` or `$N` for Postgres) and bind a value to it.
    ///
    /// ### Note: Database-specific Limits
    /// Note that every database has a practical limit on the number of bind parameters
    /// you can add to a single query. This varies by database.
    ///
    /// While you should consult the manual of your specific database version and/or current
    /// configuration for the exact value as it may be different than listed here,
    /// the defaults for supported databases as of writing are as follows:
    ///
    /// * Postgres and MySQL: 65535
    ///     * You may find sources that state that Postgres has a limit of 32767,
    ///       but that is a misinterpretation of the specification by the JDBC driver implementation
    ///       as discussed in [this Github issue][postgres-limit-issue]. Postgres itself
    ///       asserts that the number of parameters is in the range `[0, 65535)`.
    /// * SQLite: 32766 (configurable by [`SQLITE_LIMIT_VARIABLE_NUMBER`])
    ///     * SQLite prior to 3.32.0: 999
    /// * MSSQL: 2100
    ///
    /// Exceeding these limits may panic (as a sanity check) or trigger a database error at runtime
    /// depending on the implementation.
    ///
    /// [`SQLITE_LIMIT_VARIABLE_NUMBER`]: https://www.sqlite.org/limits.html#max_variable_number
    /// [postgres-limit-issue]: https://github.com/launchbadge/sqlx/issues/671#issuecomment-687043510
    pub fn push_bind<T>(&mut self, value: T) -> &mut Self
    where
        T: 'args + Encode<'args, DB> + Type<DB>,
    {
        self.sanity_check();

        let arguments = self
            .arguments
            .as_mut()
            .expect("BUG: Arguments taken already");
        arguments.add(value).expect("Failed to add argument");

        arguments
            .format_placeholder(&mut self.query)
            .expect("error in format_placeholder");

        self
    }

    pub fn push_fragment(&mut self, fragment: QueryBuilder<'args, DB>) -> &mut Self {
        self.sanity_check();

        let arguments = self
            .arguments
            .as_mut()
            .expect("BUG: Arguments taken already");
        let fragment_arguments = fragment
            .arguments
            .take()
            .expect("BUG: Arguments taken already");

        self.query.push_str(&fragment.query);

        arguments.reserve(fragment_arguments.len(), 0);
        for argument in fragment.arguments {
            arguments.add(argument).expect("Failed to add argument");
        }

        arguments
            .format_placeholder(&mut self.query)
            .expect("error in format_placeholder");

        self
    }

    /// Start a list separated by `separator`.
    ///
    /// The returned type exposes identical [`.push()`][Separated::push] and
    /// [`.push_bind()`][Separated::push_bind] methods which push `separator` to the query
    /// before their normal behavior. [`.push_unseparated()`][Separated::push_unseparated] and [`.push_bind_unseparated()`][Separated::push_bind_unseparated] are also
    /// provided to push a SQL fragment without the separator.
    ///
    /// ```rust
    /// # #[cfg(feature = "mysql")] {
    /// use sqlx::{Execute, MySql, QueryBuilder};
    /// let foods = vec!["pizza".to_string(), "chips".to_string()];
    /// let mut query_builder: QueryBuilder<MySql> = QueryBuilder::new(
    ///     "SELECT * from food where name in ("
    /// );
    /// // One element vector is handled correctly but an empty vector
    /// // would cause a sql syntax error
    /// let mut separated = query_builder.separated(", ");
    /// for value_type in foods.iter() {
    ///   separated.push_bind(value_type);
    /// }
    /// separated.push_unseparated(") ");
    ///
    /// let mut query = query_builder.build();
    /// let sql = query.sql();
    /// assert!(sql.ends_with("in (?, ?) "));
    /// # }
    /// ```

    pub fn separated<'qb, Sep>(&'qb mut self, separator: Sep) -> Separated<'qb, 'args, DB, Sep>
    where
        'args: 'qb,
        Sep: Display,
    {
        self.sanity_check();

        Separated {
            query_builder: self,
            separator,
            push_separator: false,
        }
    }

    fn into_sqlx_query_builder(self) -> sqlx::QueryBuilder<'args, DB> {
        let arguments = ArgumentsWrapper(self.arguments.unwrap());
        sqlx::QueryBuilder::with_arguments(self.query, arguments)
    }

    /// Reset this `QueryBuilder` back to its initial state.
    ///
    /// The query is truncated to the initial fragment provided to [`new()`][Self::new] and
    /// the bind arguments are reset.
    pub fn reset(&mut self) -> &mut Self {
        self.query.truncate(self.init_len);
        self.arguments = Some(Default::default());

        self
    }

    /// Get the current build SQL; **note**: may not be syntactically correct.
    pub fn sql(&self) -> &str {
        &self.query
    }

    /// Deconstruct this `QueryBuilder`, returning the built SQL. May not be syntactically correct.
    pub fn into_sql(self) -> String {
        self.query
    }
}

/// A wrapper around `QueryBuilder` for creating comma(or other token)-separated lists.
///
/// See [`QueryBuilder::separated()`] for details.
#[allow(explicit_outlives_requirements)]
pub struct Separated<'qb, 'args: 'qb, DB, Sep>
where
    DB: Database,
{
    query_builder: &'qb mut QueryBuilder<'args, DB>,
    separator: Sep,
    push_separator: bool,
}

impl<'qb, 'args: 'qb, DB, Sep> Separated<'qb, 'args, DB, Sep>
where
    DB: Database,
    Sep: Display,
{
    /// Push the separator if applicable, and then the given SQL fragment.
    ///
    /// See [`QueryBuilder::push()`] for details.
    pub fn push(&mut self, sql: impl Display) -> &mut Self {
        if self.push_separator {
            self.query_builder
                .push(format_args!("{}{}", self.separator, sql));
        } else {
            self.query_builder.push(sql);
            self.push_separator = true;
        }

        self
    }

    /// Push a SQL fragment without a separator.
    ///
    /// Simply calls [`QueryBuilder::push()`] directly.
    pub fn push_unseparated(&mut self, sql: impl Display) -> &mut Self {
        self.query_builder.push(sql);
        self
    }

    /// Push the separator if applicable, then append a bind argument.
    ///
    /// See [`QueryBuilder::push_bind()`] for details.
    pub fn push_bind<T>(&mut self, value: T) -> &mut Self
    where
        T: 'args + Encode<'args, DB> + Type<DB>,
    {
        if self.push_separator {
            self.query_builder.push(&self.separator);
        }

        self.query_builder.push_bind(value);
        self.push_separator = true;

        self
    }

    /// Push a bind argument placeholder (`?` or `$N` for Postgres) and bind a value to it
    /// without a separator.
    ///
    /// Simply calls [`QueryBuilder::push_bind()`] directly.
    pub fn push_bind_unseparated<T>(&mut self, value: T) -> &mut Self
    where
        T: 'args + Encode<'args, DB> + Type<DB>,
    {
        self.query_builder.push_bind(value);
        self
    }
}

pub struct ArgumentsWrapper<'q, DB: Database>(pub <DB as Database>::Arguments<'q>);

impl<'q, DB: Database> IntoArguments<'q, DB> for ArgumentsWrapper<'q, DB> {
    fn into_arguments(self) -> <DB as Database>::Arguments<'q> {
        self.0
    }
}
