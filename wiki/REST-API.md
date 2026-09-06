> **位置づけ:** 非規範的な索引です。正本は[`REST_API_V1.md`](https://github.com/salty919/codex_info_v2/blob/main/docs/REST_API_V1.md)です。ファイル名にv1が残りますが、同文書がv1/v2/v3全世代のWIRE ownerです。

# REST API

## version別構造

master IDは`API-LIFECYCLE-01`、`API-V3-MODELS-01`、`API-DEPRECATION-01`、`REST-129`、`WIN-PARITY-WIRE-01`、`WIN-PARITY-PAIR-01`です。実装入口は`src/server.rs`、Linux clientは`src/main.rs`、Windows clientは`windows-client/src/CodexInfo.WindowsClient.Core/LoopbackStatusClient.cs`です。

| route | 状態 | 用途 |
| --- | --- | --- |
| `GET /health` | stable | service readinessとproduct version。認証済み・最新収集成功を意味しない |
| `GET /v1/health` | compatibility | `/health`と同じreadiness契約 |
| `GET /v3/details` | current | 任意model配列、model-source provenance、cache writeを含む単一表示root |
| `GET /v2/details` | deprecated | 固定3モデル＋provenanceの互換projection |
| `GET /v1/details` | deprecated | 固定3モデルの旧互換projection |

v1/v2/v3は別DBではなく、同じcommit済みdomain snapshotから生成するread-only adapterです。旧adapterの廃止対象はroute、adapter、client fallbackだけで、collector、DB writer、Session、stable healthは変更しません。Sunset日は未決定です。

## client受理

1. v3を要求する。
2. exact 404だけでv2へ進む。
3. v2もexact 404だけでv1へ進む。
4. timeout、別status、header/body/schema不正ではfallbackせずlast-goodを保持する。

Linux/Windowsとも一つのdetails応答と`Codex-Info-Published-Pair`をatomic rootとして受理します。複数version、SQLite row、control応答をmergeしません。schema-validならclientとdaemonのproduct version不一致だけで停止しません。

## read-only境界

公開routeはloopback `127.0.0.1:8787`限定です。GETを含む全requestはSQLite transaction、WAL/SHM、migration、prune、backup、published generationを変更しません。既知pathのnon-GETは405、未知・case違い・末尾slash・query付きpathは404です。responseはJSON、`Cache-Control: no-store`、有限header/body/array上限を持ちます。

## 直接oracle

route/schema/header/effectは`src/server.rs`のserver test、Linux fallbackは`src/main.rs`のdetails v3 case、WindowsはCore contract testで直接確認します。API説明の正否はUI画像ではなく、正本のexact schemaと実route/client parserを照合します。

## 未確認の扱い

current revisionのexact route/schema/effectと両client parserを直接確認していない状態は`INCONCLUSIVE`です。daemon/clientのproduct version一致、UI画像、旧adapterの成功だけでcurrent v3互換を合格にしません。
