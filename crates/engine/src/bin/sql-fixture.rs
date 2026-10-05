//! Bounded differential fixture driver, never a production server.
use std::{
    io::{self, Read},
    sync::atomic::{AtomicU64, Ordering},
};
use vetra_engine::{
    Database, Limits, Lineage,
    sql::{Access, Scalar, Session},
};
use vetra_types::sql::serde_json;
fn main() {
    if let Err(e) = run() {
        eprintln!("fixture failed: {e}");
        std::process::exit(1)
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let mut input = String::new();
    io::stdin()
        .take(1024 * 1024 + 1)
        .read_to_string(&mut input)?;
    if input.len() > 1024 * 1024 {
        return Err("fixture budget".into());
    }
    let workload: Vec<String> = serde_json::from_str(&input)?;
    let path = std::env::temp_dir().join(format!(
        "vetra-sql-fixture-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
    }
    let path = path.canonicalize()?;
    let db = Database::open(
        &path,
        Lineage {
            database: [1; 16],
            timeline: [2; 16],
        },
        Limits::default(),
    )
    .map_err(|e| format!("{e:?}"))?;
    let mut s = Session::new(
        db.transactions().clone(),
        1,
        Access {
            admin: true,
            ..Access::default()
        },
    );
    let mut results = vec![];
    for sql in workload {
        let value = match s.execute(&sql, &[], 1_700_000_000_000_000) {
            Ok(results) => {
                let r = results.last().map(|o| &o.relation);
                serde_json::json!({"rows":r.map(|r|r.rows.iter().map(|row|row.iter().map(|v|if v==&Scalar::Null{None}else{Some(v.text())}).collect::<Vec<_>>()).collect::<Vec<_>>()).unwrap_or_default(),"oids":r.map(|r|r.columns.iter().map(|c|c.ty.oid()).collect::<Vec<_>>()).unwrap_or_default()})
            }
            Err(e) => serde_json::json!({"error":e.code}),
        };
        results.push(value);
    }
    drop(s);
    drop(db);
    std::fs::remove_dir_all(path)?;
    println!("{}", serde_json::to_string(&results)?);
    Ok(())
}
