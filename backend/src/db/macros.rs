// Wraps the body in a closure returning Result so rollback always runs before an
// error propagates (including `?` early returns). Takes an owned `sqlx::Transaction`
// so `.commit()`/`.rollback()` consume it, preventing use-after-close.
macro_rules! with_transaction {
    ($pool:expr, |mut $tx:ident| $body:block) => {{
        // BEGIN IMMEDIATE (not the default deferred BEGIN): our write transactions
        // read before they write, and under WAL a deferred transaction's read
        // snapshot can go stale if another connection commits in between — SQLite
        // then rejects the write-lock upgrade with SQLITE_BUSY_SNAPSHOT (517), which
        // `busy_timeout` does NOT retry, failing with "database is locked". Taking the
        // write lock up front removes that upgrade path; a concurrent writer is waited
        // out via busy_timeout and the snapshot can never go stale.
        let mut $tx = $pool.begin_with("BEGIN IMMEDIATE").await?;
        let res = async { $body }.await;
        match res {
            Ok(v) => {
                $tx.commit().await?;
                Ok(v)
            }
            Err(e) => {
                let _ = $tx.rollback().await;
                Err(e)
            }
        }
    }};
}

// Three variants because sqlx cannot abstract over the number of bind parameters
// (0, 1, many). Each ensures the `?` on execute() is never forgotten.
macro_rules! exec {
    ($self:ident, $query:expr) => {{
        sqlx::query($query).execute(&$self.pool).await?;
        Ok(())
    }};
}

macro_rules! exec_by_id {
    ($self:ident, $query:expr, $id:expr) => {{
        sqlx::query($query).bind($id).execute(&$self.pool).await?;
        Ok(())
    }};
}

macro_rules! exec_with_bindings {
    ($self:ident, $query:expr, $($bind:expr),+) => {{
        let mut q = sqlx::query($query);
        $(q = q.bind($bind);)+
        q.execute(&$self.pool).await?;
        Ok(())
    }};
}
