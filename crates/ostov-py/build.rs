// Без OSTOV_PG_URL тесты PostgreSQL помечаются ignored, а не проходят молча.
fn main() {
    println!("cargo::rustc-check-cfg=cfg(pg_url)");
    println!("cargo::rerun-if-env-changed=OSTOV_PG_URL");
    if std::env::var_os("OSTOV_PG_URL").is_some() {
        println!("cargo::rustc-cfg=pg_url");
    }
}
