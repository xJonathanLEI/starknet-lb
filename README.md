<p align="center">
  <h1 align="center">starknet-lb</h1>
</p>

**Pending block-aware Starknet-native JSON-RPC load balancer**

## Introduction

Running multiple Starknet full nodes but having trouble making sure requests don't hit staled instances? `starknet-lb` is here to rescue! Built from scratch for Starknet, `starknet-lb` is aware of Starknet's `PENDING` block to make the most informed decision on traffic distribution.

> [!NOTE]
>
> `starknet-lb` is in early development. Currently, it only sends traffic to instances with the most up-to-date pending block, ensuring access to the latest network state at the cost of resource under-utilization. This should be improved in the future with configurable, context-based stalenss policy that's able to utilize staled instances to serve historical data (e.g. an instance that's 10 blocks behind chain head should have no problem serving a `starknet_call` request on a block from 5 days ago).

## Installation

Install the crate directly from [crates.io](https://crates.io/crates/starknet-lb):

```console
cargo install --locked starknet-lb
```

Alternatively, use the pre-built Docker images from [Docker Hub](https://hub.docker.com/r/starknet/starknet-lb).

## Upstream configuration

Currently, the load balancer can only be configured with command line options. To set an upstream, use `--upstream <SPEC>`, which can be used multiple times to add multiple upstream specifications.

Two types of upstreams are supported:

1. Raw URL: Simply supply the full URL of the upstream endpoint as the value. For example: `--upstream https://starknet-mainnet.public.blastapi.io/rpc/v0_7`.

2. DNS: Use the `dns:` prefix to set a DNS name that will be resolved into actual hosts. This can be very useful in environments involving auto-scaling (e.g. Kubernetes), where the list of upstream hosts is not static.

   An simple example of setting a DNS upstream is `--upstream dns:pathfinder.default.svc.cluster.local`.

   You can optionally set a port number if the upstream nodes are not serving from the default port of `9545`. For example: `--upstream dns:pathfinder.default.svc.cluster.local:8080`.

   It's also possible to specify a URL path to apply to all the resolved hosts. To use a path of `/rpc/v0_7`: `--upstream dns:pathfinder.default.svc.cluster.local/rpc/v0_7`.

   The two optional components can also be combined. Here's an example of using both in the same specification: `--upstream dns:pathfinder.default.svc.cluster.local:8080/rpc/v0_7`.

> [!NOTE]
>
> The load balancer is a work in progress. Specifically, DNS hostnames are only resolved at startup and are not continuously monitored. This should be implemented in the future.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](./LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](./LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
