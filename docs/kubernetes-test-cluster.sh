#!/usr/bin/env bash
# A local Kubernetes cluster (k3s in Docker) to try Kubernetes connections against.
# See docs/kubernetes-test-cluster.md. Writes its kubeconfig to target/k8s-test/kubeconfig.
set -euo pipefail
out="$(cd "$(dirname "$0")/.." && pwd)/target/k8s-test"
k() { docker exec -i cmk3s kubectl "$@"; }

docker rm -f cmk3s >/dev/null 2>&1 || true
docker run -d --name cmk3s --privileged --dns 1.1.1.1 -p 16443:6443 rancher/k3s:latest \
  server --disable traefik --disable metrics-server --tls-san 127.0.0.1 >/dev/null
echo "waiting for k3s…"
until k get serviceaccount default >/dev/null 2>&1; do sleep 2; done

until k apply -f - >/dev/null 2>&1 <<'EOF'
apiVersion: v1
kind: Namespace
metadata: {name: cache}
---
apiVersion: v1
kind: Namespace
metadata:
  name: strict
  labels: {pod-security.kubernetes.io/enforce: restricted, pod-security.kubernetes.io/enforce-version: latest}
---
apiVersion: apps/v1
kind: StatefulSet
metadata: {name: rc, namespace: cache}
spec:
  serviceName: rc-headless
  replicas: 3
  selector: {matchLabels: {app: rc}}
  template:
    metadata: {labels: {app: rc}}
    spec:
      containers:
      - name: redis
        image: redis:7-alpine
        args: ["redis-server", "--cluster-enabled", "yes", "--requirepass", "s3cret", "--masterauth", "s3cret", "--protected-mode", "no"]
        ports: [{name: redis, containerPort: 6379}]
        readinessProbe: {tcpSocket: {port: 6379}}
---
apiVersion: v1
kind: Service
metadata: {name: rc-headless, namespace: cache}
spec: {clusterIP: None, selector: {app: rc}, ports: [{port: 6379}]}
---
apiVersion: v1
kind: Service
metadata: {name: redis-sel, namespace: cache}
spec: {selector: {app: rc}, ports: [{port: 6379, targetPort: redis}]}
---
apiVersion: v1
kind: Pod
metadata: {name: relay, namespace: cache}
spec:
  containers: [{name: relay, image: alpine:3, command: ["sleep", "infinity"]}]
EOF
do sleep 2; done
echo "waiting for the pods…"
until k -n cache get pod rc-2 >/dev/null 2>&1; do sleep 2; done
k -n cache wait --for=condition=ready pod/rc-0 pod/rc-1 pod/rc-2 pod/relay --timeout=300s >/dev/null

ips=$(k -n cache get pods -l app=rc -o jsonpath='{range .items[*]}{.status.podIP} {end}')
k -n cache exec rc-0 -- sh -c "redis-cli -a s3cret --no-auth-warning --cluster create $(for ip in $ips; do printf '%s:6379 ' "$ip"; done) --cluster-replicas 0 --cluster-yes" | tail -1

# the Service "redis" has no selector: its endpoints are set by hand
addresses=$(for ip in $ips; do printf '{ip: %s}, ' "$ip"; done)
k apply -f - >/dev/null 2>&1 <<EOF
apiVersion: v1
kind: Service
metadata: {name: redis, namespace: cache}
spec: {ports: [{port: 6379}]}
---
apiVersion: v1
kind: Endpoints
metadata: {name: redis, namespace: cache}
subsets: [{addresses: [${addresses%, }], ports: [{port: 6379}]}]
EOF

mkdir -p "$out"
docker exec cmk3s cat /etc/rancher/k3s/k3s.yaml \
  | sed -E 's#https://127.0.0.1:6443#https://127.0.0.1:16443#; s/^(\s*-?\s*(name|cluster|user|current-context): )default$/\1cachemire-test/' \
  | sed -E 's/^(    user: cachemire-test)$/\1\n    namespace: cache/' > "$out/kubeconfig"
chmod 600 "$out/kubeconfig"
echo "ready: context cachemire-test, namespace cache, nodes $ips"
echo "start the app with: KUBECONFIG=$out/kubeconfig target/fast/cachemire"
