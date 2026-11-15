# maat-cli

The command-line front end for the [Ma'at](https://github.com/IsomorphicAlgo/Ma-at) contact
scheduler. It installs a `maat` command and is a thin layer over `maat-ground`.

```text
maat plan            Build a contact plan from a satellite catalog and a station file
maat codes           Print the MAAT-xxx reason-code table
maat explain <CODE>  Explain one reason code and what would change the outcome
maat --version       Print the version
maat --help          Print help
```

## `maat plan`

```text
maat plan --catalog <FILE> --stations <FILE> [--requests <FILE>]
          [--start 2026-08-01T12:00:00Z] [--hours 24]
          [--scheduler greedy|optimal] [--time-limit <SECONDS>] [--floor]
          [--format text|json|satnogs|station] [--station <ID>] [--output <FILE>]
```

| Option | Meaning | Default |
|---|---|---|
| `--catalog` | Satellites: OMM (JSON, KVN, XML, CSV) or two-line element sets; detected | required |
| `--stations` | TOML file of `[[station]]` tables; may also hold `[[request]]` tables | required |
| `--requests` | A second TOML file, usually of `[[request]]` tables | none |
| `--start` | Start of the search, with `Z` or an offset | now |
| `--hours` | Length of the search | 24 |
| `--scheduler` | `greedy` or `optimal` | `greedy` |
| `--time-limit` | Longest the optimal scheduler may search, in seconds | 60 |
| `--floor` | Optimal only: no satellite gets less than the greedy plan gives it | off |
| `--format` | `text`, `json`, `satnogs` or `station` | `text` |
| `--station` | The station to write with `--format station` | the only one |
| `--output` | Write to a file | standard output |

The plan goes to standard output. Notes and warnings go to standard error. Exit status is 0 for
a plan, 1 when the inputs could not be turned into one, and 2 for a bad command line.

## Station and request files

```toml
[[station]]
id = 1                          # your own number for the station, 0 to 65535
name = "Seattle"
latitude_deg = 47.61            # north positive
longitude_deg = -122.33         # east positive, -180 to 180
altitude_m = 50.0               # optional, default 0
minimum_elevation_deg = 10.0    # the elevation mask
turnaround_s = 30               # optional: fixed time between passes
slew_azimuth_deg_per_s = 6.0    # optional, both or neither: how fast the antenna turns
slew_elevation_deg_per_s = 2.5
satnogs_id = 2433               # optional: the station's number on the SatNOGS Network

[[request]]
satellite = 25544               # catalog number, as in the catalog file
priority = 5                    # 0 to 255, higher wins
min_contact_s = 180             # optional: ignore passes shorter than this
need_s = 1200                   # optional: stop scheduling once this much is planned
satnogs_transmitter = "AbCdEfGhJkLmNpQrStUvWx"   # optional: for the SatNOGS export
```

A key that is not listed is an error and is reported with its line. A working pair of files is
in [`examples/`](../../examples) at the top of the repository.

## Limits

- `--format satnogs` matches the SatNOGS Network's documented request for new observations. It
  has not been sent to the network.
- `--format station` is written for a Chronus Gateway station. Chronus Gateway 1.0.0-rc.1 has no
  schedule input, so nothing reads it yet.
- The warning about old element sets (more than 14 days from the search) is a rule of thumb.

License: MIT.
