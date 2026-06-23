mod crud;
mod discover;
mod maintenance;
mod query;
mod schema;
mod sync;

use crate::Command;

pub(crate) fn run(cli: crate::Cli) -> ddb_core::error::Result<()> {
    let repo = &cli.repo;
    match cli.command {
        Command::Help { topic } => query::help(topic),
        Command::Init { path } => crud::init(repo, path),
        Command::Create {
            title,
            tags,
            r#type,
            body,
            set,
        } => crud::create(repo, title, tags, r#type, body, set),
        Command::Read { id } => crud::read(repo, &id),
        Command::Update {
            id,
            title,
            tags,
            r#type,
            body,
            set,
            unset,
        } => crud::update(
            repo,
            crud::UpdateArgs {
                id,
                title,
                tags,
                r#type,
                body,
                set,
                unset,
            },
        ),
        Command::Delete { id } => crud::delete(repo, &id),
        Command::Sync { remote, branch } => sync::sync(repo, &remote, &branch),
        Command::Query { sql } => query::query(repo, &sql),
        Command::Search {
            query,
            limit,
            offset,
        } => query::search(repo, &query, limit, offset),
        Command::RegisterNode { name } => sync::register_node(repo, &name),
        Command::Status => maintenance::status(repo),
        Command::Compact {
            force,
            dry_run,
            no_backup,
            backup_path,
        } => maintenance::compact(repo, force, dry_run, no_backup, backup_path),
        Command::Reindex => maintenance::reindex(repo),
        Command::Fix {
            dry_run,
            verbose,
            migrate,
        } => maintenance::fix(repo, dry_run, verbose, migrate),
        Command::Rename { id, new_path } => crud::rename(repo, &id, &new_path),
        Command::Type { action } => discover::type_cmd(repo, action),
        Command::Node { action } => sync::node(repo, action),
        Command::Bundle { action } => sync::bundle(repo, action),
        Command::Serve {
            port,
            pg_port,
            bind,
            playground,
        } => maintenance::serve(repo, port, pg_port, &bind, playground),
        Command::Attach { id, file } => crud::attach(repo, &id, &file),
        Command::Detach { id, filename } => crud::detach(repo, &id, &filename),
        Command::Attachments { id } => crud::attachments(repo, &id),
        Command::Get { id } => query::get(repo, &id),
        Command::Scan { r#type, tag } => query::scan(repo, r#type, tag),
        Command::Backlinks { id } => query::backlinks(repo, &id),
        Command::Maintenance { action } => maintenance::maintenance(repo, action),
        Command::Discover { action } => discover::discover(repo, action),
        Command::Sequence { action } => discover::sequence(repo, action),
        Command::Schema { action } => schema::schema(repo, action),
        // Handled in main() before run() is called
        Command::UpdateBin { .. } | Command::UpdateCheck => unreachable!(),
    }
}
