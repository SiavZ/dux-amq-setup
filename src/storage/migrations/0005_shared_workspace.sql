-- Shared-workspace schema foundation. The migration runner splits this file
-- at the marker so Rust can derive deterministic agent handles inside the
-- same transaction as the table rebuild and user_version bump.

create table agent_sessions_new (
    id text primary key,
    project_id text not null,
    provider text not null,
    source_branch text not null,
    branch_name text not null,
    worktree_path text not null,
    title text,
    project_path text,
    started_providers text not null default '[]',
    status text not null,
    created_at text not null,
    updated_at text not null,
    state_json text,
    session_settings text,
    sort_order integer not null default 0,
    shared_workspace integer not null default 0,
    agent_handle text not null unique check (
        agent_handle glob '[a-z0-9_-]*'
        and agent_handle not glob '*[^a-z0-9_-]*'
        and length(agent_handle) between 1 and 64
    ),
    deleted_at text
);

-- rust-backfill-agent-handles

create table session_prs_new (
    session_id text not null,
    pr_number integer not null,
    owner_repo text not null,
    state text not null default 'OPEN',
    title text not null default '',
    primary key (session_id, pr_number),
    foreign key (session_id) references agent_sessions_new(id) on delete cascade
);

-- Copy only PR rows whose session still exists. A legacy DB could hold an
-- orphaned session_prs row (session gone without its cascade firing); copying
-- it would abort the whole migration under foreign_keys=ON and brick TUI
-- launch. Orphan PR associations are already meaningless, so drop them.
insert into session_prs_new (session_id, pr_number, owner_repo, state, title)
select session_id, pr_number, owner_repo, state, title
from session_prs
where session_id in (select id from agent_sessions_new);

drop table session_prs;
drop table agent_sessions;
alter table agent_sessions_new rename to agent_sessions;
alter table session_prs_new rename to session_prs;

create index idx_agent_sessions_sort_order
    on agent_sessions(sort_order, updated_at desc, id);
