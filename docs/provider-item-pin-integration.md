# Provider item-ID native pin composition

Base `52c46d1a3d7ef1a6baf9d2ab5caaa8fe1ad45d6e`, tree
`e081f24f9b81d37d53857e691a99d49679b80834`; clean before edits. Production
descendant `d89a1585bb9d2967a1078fa95019c8023af81297`, tree
`66136b19cd24c1e5046bf2139d1e30418254cf3a` contains only the exact native pins,
provenance label and focused rejection assertions. Later commits are test/docs.

Default artifacts and composed manifest:
`/mnt/mambo/PersonaProjects/cutex-mcp-facade-r1/artifacts/provider-item-pin-r1/`.
`bin/cutex` SHA256 `15e2fdd9135222645608cb2539d8c589632029f86e85463147f8b4387edb6ccb`;
`bin/cutex-mcp` SHA256 `c08772db83b8c89271ed35ae207f39a19ce9eda1ef045b75b918a8b93fa948e0`.

Native ca580a783fc1ab34613be4f81ceab96ef4d393a2 / tree
81b6400b6858ac4f042f6b069ecad5b19177323b, upstream bundle manifest SHA256
ddf1ee5133ff25624b9175c18e431b4c2eb2c7bde04853c1b8eebe3d51e1822c.
Full companion hashes are in build-manifest.json; native bundle checksums and
transferred CLI/server/host/schema/Cutex/facade bytes were independently checked.
Job f3bc9c8/d98d5e33 is unchanged and not exercised by the readiness test.

## Compatibility

New v3 reviews accept exactly ca580a78 CLI/server with unchanged official host
and schema. Previous coherent 0c and a83 bundles, individually mixed old
components, spoofed executable/host/schema reject; there is no wildcard or K
fallback. Official stock v1 and earlier c2 v2 rules remain unchanged. Wire,
marker and receipt schemas are unchanged. No producer jsc shortening,
envelope/history rewrite or receipt mutation. Native repair omits oversized
optional IDs in the outbound request copy, not persisted business identities.

Existing hj1/r3/r4 owners, files, holds and credentials were not touched.
Their old markers still identify old bytes; changing a file in place is NOT an
upgrade. Current activation rejects already-activated records (no downgrade or
marker replacement API). Therefore restarting hj1 with a changed manifest is
not a supported shortcut. A new isolated reviewed subject is the bounded next
Human test option; same-history upgrade needs a separately authorized explicit
compatibility path. This task does not add one.

## Checks

- Default `cargo test --locked --lib launch::stock::tests`: 6 passed.
- `cargo test --locked --lib bootstrap`: 8 passed.
- `cargo test --locked --bin cutex stock_lifecycle`: 2 passed.
- Default `cargo build --locked --bin cutex --bin cutex-mcp`: passed.
- `cargo fmt --check`, scoped diff check and probe AST: passed.
- Existing unrelated unused/dead-code warnings retained; no dependency changes.
- Native owner's 23 client / 6 protocol tests and model-free host/CLI checks
  reused, not rerun. No full workspace, sandbox, PTY, Windows or paid campaign.

Private executable probe: `scripts/provider_item_pin_smoke.py`, reusing exact
base-fixture ed1ebdb5 by hash. Fresh `pi1` only, fake provider configuration,
neutral thread persistence, root review/activation/replay, nonroot activation
denial, real registration/readiness, same native ID and zero turns. Subprocess
connect tripwire remains, distinct owned ports; no claim of hostile-child OS
isolation. Cleanup is restricted to owned fixture children and exact binding.
First attempt p1 failed before Bus readiness: fixture chose port25300 outside
required24xxx range. Its logs/history remain, zero model requests. Corrected
fixture uses free249xx ports; no policy or production code workaround.

Corrected p2 passed on the final default bytes: real provider activation replay
and wrong-root-token401, Ready generation1, durable
`cutex.01a08d4b-65e1-7c11-aad3-03989ab15cdb`, same native UUID, positive v1/Soon
handshake, neutral history with zero turns and zero model calls. Independent
store readback equals the Ready binding. `VM-RESULT.json` and `vm-p2.log` in the
artifact directory preserve the exact hashes. Owned probe cleanup completed;
ports24900/24901 no longer listen. No hj1 endpoint was selected. This fake-mode
check does not prove real aemeath, Job tools, long-ID provider acceptance or CLI
interaction on the new pair.

Composed `build-manifest.json` SHA256:
`0153e3e1a1625b6e08c3e710ba17dac675ac2ecbd71559a6535721e2c8f8d8cb`.
Final retained taskroot ~14GiB (<20), Mambo free372GiB; guest upload6.9GiB (<8),
guest free49GiB. No native rebuild, home cache, credential staging or cleanup of
unrelated evidence. Complete scoped production diff and probe reviewed.

## Human 手动下一步（不是自动启动命令）

现有 SSH 窗口和 hj1 保持不动，不再向旧 CLI 提交 Job，也不 retry/release。
新文件在 guest `/home/cutex-linux-test/acceptance-upload/pi1/`，但无模型检查
夹具不是 terra/Job 人工测试会话，不能直接运行旧 `hj1_manual.py` 指向新字节。

最短连接命令仍是：

```sh
ssh -F /mnt/mambo/vmstore/cutex-linux-acceptance/ssh/config cutex-linux-acceptance
```

接受本组合后，下一步应明确批准一个新的私有 terra-low/Job 人工测试主体，
用新 bundle 做独立审阅/激活，再按旧文档的分步服务、错误观察、同 owner
stock-attach 方式操作。认证仅由 Human 届时复制；本任务未复制认证或调用模型。
旧 owner 的停止/清理须先与 Human 协调，不能默默换二进制或改 marker。

这是 private integration candidate，不是 provider 已接受修复、其他 FCO
兼容性、完整 Job 回复或发布成功的证明。保留 S2/S46 等既有风险披露不变。
