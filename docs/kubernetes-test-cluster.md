# A local Kubernetes cluster for trying Kubernetes connections

`docs/kubernetes-test-cluster.sh` starts k3s in a Docker container (`cmk3s`, API server on
`127.0.0.1:16443`). Inside, namespace `cache` holds:

- a 3-node Redis cluster (pods `rc-0` to `rc-2`, password `s3cret`), whose nodes announce their pod IPs;
- `redis`: a Service with **no selector**, its endpoints set by hand to the three pods (port-forward can't reach it);
- `redis-sel`: a Service with a selector (port-forward reaches it);
- `relay`: an Alpine pod (busybox `nc`) to use as the relay pod.

It needs only Docker; no `kubectl` on the machine. From the repository root:

```text
docs/kubernetes-test-cluster.sh
KUBECONFIG=target/k8s-test/kubeconfig target/fast/cachemire
```

The kubeconfig (`target/k8s-test/kubeconfig`) has one context, `cachemire-test`, whose namespace is `cache`.

## What to try

Each with the password `s3cret` and "Inside a Kubernetes cluster" on (context and namespace may stay empty):

| URL | Relay pod | Expected |
|---|---|---|
| `redis://redis:6379` | `relay` | connects, as a cluster |
| `redis://redis:6379` | (empty) | the Service has no pods behind it (no selector): name a relay pod |
| `redis://redis-sel:6379` | (empty) | connects through port-forward, as a cluster |
| `redis://rc-1:6379` | (empty) | connects to that pod, as a cluster |
| `redis://redis:6390` | `relay` | the relay pod couldn't reach redis:6390 |
| `redis://redis:6379` | `nope` | Kubernetes refused: pods "nope" not found |

## Notes

- Run the script again after the container restarts: the pods get new IPs, which breaks the Redis cluster.
  It deletes and rebuilds everything each time.
- The container uses the `1.1.1.1` DNS server: with the host's resolver, k3s may fail to pull its images.
- Remove it with `docker rm -f cmk3s`.
