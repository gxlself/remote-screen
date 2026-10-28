# Test control fork

This package is copied from the `tailscale.com` module at version `v1.94.2`,
from:

```
tailscale.com/tstest/integration/testcontrol/testcontrol.go
```

The test tailnet needs the same auth-key behavior as the real control plane:
an auth key is required when a node registers for the first time, but a node
with a previously registered node key can register again without presenting
the key. The upstream test helper requires the key on every registration,
which prevents testing restart with persisted state.

The only change from the upstream file is this auth-key check:

```diff
--- tailscale.com/tstest/integration/testcontrol/testcontrol.go
+++ tailnet/internal/testcontrol/testcontrol.go
@@
-	if s.RequireAuthKey != "" && (req.Auth == nil || req.Auth.AuthKey != s.RequireAuthKey) {
+	if s.RequireAuthKey != "" {
+		s.mu.Lock()
+		_, nodeKeyRegistered := s.nodes[req.NodeKey]
+		_, oldNodeKeyRegistered := s.nodes[req.OldNodeKey]
+		s.mu.Unlock()
+		if !nodeKeyRegistered && !oldNodeKeyRegistered && (req.Auth == nil || req.Auth.AuthKey != s.RequireAuthKey) {
			res := must.Get(s.encode(false, tailcfg.RegisterResponse{
				Error: "invalid authkey",
			}))
			w.WriteHeader(200)
			w.Write(res)
			return
+		}
+	}
```

`LICENSE` is copied from the upstream repository and is the BSD-3-Clause
license. When upgrading the `tailscale.com` version, copy the upstream file
again and replay the change above, then verify that this remains the only
diff.
