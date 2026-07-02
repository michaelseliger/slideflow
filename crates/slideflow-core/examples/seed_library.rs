//! Seed a library database with a root folder and scan it.
//!
//! Usage: seed_library <db_path> <root_dir>

use slideflow_core::index::Library;
use slideflow_core::model::ScanEvent;

fn main() {
    let db = std::env::args().nth(1).expect("db path");
    let root = std::env::args().nth(2).expect("root dir");
    if let Some(parent) = std::path::Path::new(&db).parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    let mut lib = Library::open(std::path::Path::new(&db)).expect("open");
    lib.add_root(std::path::Path::new(&root)).expect("add_root");
    lib.scan(&mut |e| {
        if let ScanEvent::Finished { indexed, removed, unchanged } = e {
            println!("scan finished: indexed={indexed} removed={removed} unchanged={unchanged}");
        }
    })
    .expect("scan");
    let (decks, slides) = lib.stats().expect("stats");
    println!("library: {decks} decks, {slides} slides at {db}");
}
