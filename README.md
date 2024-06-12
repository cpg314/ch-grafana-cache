# ch-grafana-cache

This utility is meant to be used with the [Clickhouse Grafana data source](https://grafana.com/grafana/plugins/grafana-clickhouse-datasource/).

It extracts the SQL queries from a Grafana dashboard and sends them to a Clickhouse server for execution. The main use case is to perform caching of the responses, e.g. via [chproxy's caching feature](https://www.chproxy.org/configuration/caching/) or [Clickhouse's query cache](https://clickhouse.com/docs/en/operations/query-cache), to make the dashboards execute faster and with less load on the database servers.

Variables are supported, even those depending on others. The tool runs over all combinations of variables.

## Usage

```console
$ ch-grafana-cache --help
Execute Clickhouse SQL queries from a Grafana dashboard.

Call with either --grafana-url and --dashboard, or with --json

Usage: ch-grafana-cache [OPTIONS] <COMMAND>

Commands:
  print    Print SQL statements, with syntax highlighting
  execute  Execute the queries
  help     Print this message or the help of the given subcommand(s)

Options:
      --grafana-url <GRAFANA_URL>
          Base Grafana URL

          [env: GRAFANA_URL=https://grafana.corp.com/]

      --dashboard <DASHBOARD>
          Grafana dashboard id

      --json <JSON>
          Dashboard JSON file

      --theme <THEME>
          Synctect for syntax highlighting. Pass any invalid value to see the list of available themes

          [env: CH_GRAFANA_CACHE_THEME=Nord]

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version

$ ch-grafana-cache execute --help
Execute the queries

Usage: ch-grafana-cache execute [OPTIONS] --url <URL> --username <USERNAME>

Options:
      --url <URL>                        URL to the Clickhouse HTTP endpoint [env: CLICKHOUSE_URL=]
      --username <USERNAME>              Clickhouse username [env: CLICKHOUSE_USERNAME=]
      --password <PASSWORD>              [env: CLICKHOUSE_PASSWORD=]
      --variables-yaml <VARIABLES_YAML>  YAML file of the form variable_name: [ values ] to manually specify the values of some variables in the dashboard
  -h, --help                             Print help
```

Examples

```console
$ # Printing the SQL queries in the dashboard
$ ch-grafana-cache --grafana https://grafana.corp.com --dashboard mydashboard print
Variables:

...

Panels:
...

$ # Executing the SQL queries in the dashboard across all combinations
$ ch-grafana-cache --grafana https://grafana.corp.com --dashboard mydashboard execute --clickhouse http://chproxy.clickhouse.internal --username default
INFO ch_grafana_cache: Retrieving dashboard
INFO ch_grafana_cache: Retrieved dashboard 'mydashboard'
INFO ch_grafana_cache: 166 variables combinations found. Executing queries...
INFO ch_grafana_cache: Executing combination i=0 n_combinations=166
INFO ch_grafana_cache: Executed combination duration=178.932498ms size_mb=0.107275
```

## Verifying that `chproxy` caching works

- Clear the `chproxy` cache.
- Close the Grafana dashboard
- Run `ch_grafana_cache`
  - The `chproxy` logs should give a `cache miss` for every query.
- Open the Grafana dashboard.
  - The `chproxy` logs should give a `cache hit` for every query.

If the dashboard gives cache misses, printing the cache key in chproxy ([here](https://github.com/ContentSquare/chproxy/blob/2d4c2bf185cb32bc127330b6f8d8614ba4ebbe61/cache/key.go#L86)) might allow understanding the difference between the cache queries and the Grafana ones. For example, a different HTTP compression setting will result in cache misses.

## Other solutions

It does not seem possible to execute the queries without loading the Grafana front-end. For example, the [Grafana snapshot API](https://grafana.com/docs/grafana/latest/developers/http_api/snapshot/) states that it is meant to be called by the UI and requires the full dashboard payload.

An alternative implementation would be to load the front-end via a headless web-browser. This is much heavier, but simpler in several aspects (e.g. no need to reimplement templating or variable fetching). To support variables, the browser would need to interact with the page.

## Current limitations

- It is assumed that the queries do not use time range information at all.
- The Clickhouse queries are sent directly (using the HTTP interface), rather than through the Grafana data source.
- Only the `${varname}` [variable syntax](https://grafana.com/docs/grafana/latest/dashboards/variables/variable-syntax/) is supported.
- It is assumed that the Clickhouse datasources are the ones containing `clickhouse` in their name.
- The queries retrieving variables must be sent twice (once for parsing with the tabular format, once in native format for caching). The could be avoided by using the native format parsing from [klickhouse](https://docs.rs/klickhouse/latest/klickhouse/).
- It is assumed that interdependent variables are topologically sorted.
- Authentication to Grafana is not supported (but easy to add).
- ...
