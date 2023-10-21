create table if not exists "frames" (
  "id" serial not null,
  "run" character varying(128) not null,
  -- wireshark packet number
  "packet_number" integer not null,
  -- ethercat pdu index
  "index" smallint not null,
  "command" character varying(32)  not null,
  "tx_time_ns" integer not null,
  "rx_time_ns" integer not null,
  "delta_time_ns" integer not null,
  primary key ("id")
);

create index if not exists "frames_scenario" on "frames" ("run");
create index if not exists "frames_run" on "frames" ("run" text_pattern_ops);

create table if not exists "runs" (
  "id" serial not null,
  primary key ("id"),
  "date" timestamptz not null,
  -- Scenario name, like `single-thread`
  "scenario" character varying(128) not null,
  -- Run name, the long one
  "name" character varying(128) not null,
  -- PC hostname
  "hostname" character varying(128) not null,
  -- EtherCAT network time
  "propagation_time_ns" integer not null,
  "settings" json not null
);

-- Idempotent unique constraint
DO $$
begin
  begin
    alter table "runs" add constraint "runs_name" unique ("name");
  exception
    when duplicate_table then  -- postgres raises duplicate_table at surprising times. ex.: for unique constraints.
    when duplicate_object then
      raise notice 'unique constraint already exists';
  end;
end $$;

create table if not exists "cycles" (
  "id" serial not null,
  primary key ("id"),
  "run" character varying(128) not null,
  "cycle" integer not null,
  "processing_time_ns" integer not null,
  "tick_wait_ns" integer not null,
  "cycle_time_delta_ns" integer not null
);

create index if not exists "cycles_scenario" on "cycles" ("run");

alter table "frames"
add foreign key ("run") references "runs" ("name") on delete cascade on update no action;

alter table "cycles"
add foreign key ("run") references "runs" ("name") on delete cascade on update no action;
