# 持久 server 安全边界

本文档把持久 server 子系统（`crates/oxide_cli/src/server/`）的安全边界定稿为四个口径：
认证模型、绑定地址、token 生命周期、防滥用。整体姿态是双闸门：loopback 绑定加本地
文件权限，token 是连接认证的唯一凭据，不经网络分发。文中实现对照一律用函数名与
常量名，具体数值以代码为准。

## 认证模型

server 的认证门在连接首帧：任何连接必须先发 `Auth` 请求帧且其中 token 与 server 的
共享 token 匹配，server 才写回 `AuthOk` 并进入主循环；首帧非 `Auth` 或 token 不匹配
时直接断连、不回帧，不向攻击者泄露差异。门在 `handle_connection`（`server.rs`），
判等函数 `tokens_match`（同文件）逐字节异或累加，避免逐字节短路的时序侧信道；长度
不等直接判不等（生产 token 长度固定 64 位十六进制）。

token 由 `generate_token`（`sidecar.rs`）生成：读 `/dev/urandom` 32 字节转 64 位小写
十六进制（256 位熵）；非 Linux 宿主回落时间加进程号混合熵（本宿主恒为 Linux，回落臂
仅保编译）。

token 分发有两条路径。持久 server 经 `write_exclusive` 把 token 写入 sidecar 文件
（写入后显式置 0600，仅属主可读写），控制客户端从 sidecar 读回 token 完成认证。
`--rm` 独立模式不写 sidecar，token 打印到前台标准输出，客户端经 CLI 旗标注入。

交接路径：新 server 接管时先读旧 sidecar 的 token，先发认证帧再发 yield 请求；旧
版本 server 无 token（空串）时跳过认证帧，保证升级路径可用（`yield_and_takeover`，
`server.rs`）。探活连接（只建连接不发帧）在认证门读首帧得 EOF 直接退出，不触发
任何错误路径。

## 绑定地址：loopback 与 0.0.0.0

缺省绑定地址是 loopback（127.0.0.1）：`ServerConfig` 的 `bind_addr` 字段缺省取
`WELL_KNOWN_BIND_ADDR`，CLI 的 `--host` 旗标缺省 127.0.0.1、`--port` 旗标缺省取
well-known 端口 `WELL_KNOWN_PORT`（4242）。安全模型假设 loopback 加本地文件权限
（sidecar 0600）双闸门，token 本身不经网络分发。

`--host 0.0.0.0` 可以配置出 0.0.0.0 绑定，本文档明确定稿该姿态为不受支持：它把
eval 端点暴露到网络，token 需经带外安全渠道分发，不在支持范围内。理由是双闸门
（loopback 加文件权限）同时失效，防护责任转移到带外分发渠道，而该渠道由使用方
自备、本项目不提供。

遗留 Unix socket 臂的门控是 socket 文件权限（操作系统用户权限），缺省姿态同样是
只接受本机连接。

## token 生命周期

token 生命周期与 server 进程同长：启动时（认领 sidecar 之前）生成，写入 sidecar
（0600），有效期等于 server 生命周期，无轮换机制；关闭序列末尾删除 sidecar 文件
即 token 销毁。

交接时新 server 生成自己的新 token，旧 token 仅在 yield 握手阶段使用一次。`--rm`
模式启动时生成、打印到标准输出、随进程退出销毁，不写入任何文件。

## 防滥用口径

防滥用共四条口径，外加执行上限：

- 并发连接上限：`MAX_CONCURRENT_CONNECTIONS`（64，`server.rs`），`accept_loop` 派生
  连接线程前查在途计数，超限连接立即关闭（不发协议帧，客户端表现为连接建立随即
  被关）。
- 有界背压：每 worker 的任务通道为有界 `sync_channel`（`QUEUE_CAPACITY`，
  `workers.rs`，容量 8），满则背压；路由在锁内克隆发送端、锁外投递，防满队列
  持锁阻塞串行卡死。
- 帧大小上限：`MAX_FRAME_BYTES`（1 MiB，`protocol.rs`），超长帧按协议约定关连接。
- 读超时：持久 server 30 秒、`--rm` 模式取空闲超时（`RM_DEFAULT_IDLE_TIMEOUT`，
  缺省 30 秒），半开连接不永久占线程。

执行上限：eval 请求带 `max_steps` 步数上限；控制请求不占 worker，不被长 eval 阻塞。

## 分片安全边界

远程分片（test262 runner 的 `--remote-supervise` 监督者与 `--worker` 执行半区）
的安全边界定稿为三个口径，与 server 子系统同型双闸门：loopback 绑定加 token
认证门。与 server 的差别在威胁模型与分发渠道：worker 执行不可信测试内容，
威胁对象是远程指派者而非本机邻居；token 不经网络分发，靠前台打印加带外注入。

### 缺省绑定地址

`--remote-supervise` 端点只允许 loopback 绑定：用户只给端口（裸端口或
`:port` 形态）时解析期补缺省 127.0.0.1；显式非 loopback 地址（0.0.0.0、
局域网 IP）在解析期报错拒绝，bind 处另有双保险拒绝非 loopback。该姿态比
server 的「0.0.0.0 可配置但不受支持」更严：server 的威胁模型是本机邻居，
worker 执行不可信测试内容，非 loopback 绑定会把窗口指派面（含本地路径
执行）暴露到网络，防护责任无法转给使用方。端点只接受 `[IP]:port` 形态，
主机名不解析（解析期无法验证 loopback）。

### worker 认证 token

监督者启动时生成 64 位十六进制 token（读 `/dev/urandom` 32 字节，非 Linux
回落时间加进程号混合熵）并打印到前台 stderr；worker 进程经
`OXIDE_TEST262_WORKER_TOKEN` 环境变量取得（带外注入，与 server `--rm`
模式同型：token 不经网络分发）。worker 连接后首帧必须是 `Auth` 事件帧，
监督者逐字节异或累加判等（长度不等直接判不等，避免逐字节短路的时序
侧信道），匹配才写回 `AuthOk` 命令帧并进窗口循环；不匹配或首帧非 `Auth`
直接断连、不回帧，不向攻击者泄露差异。读首帧带 30 秒读超时，半开连接
不永久占线程。

token 生命周期与监督者进程同长：启动时生成、进程退出即销毁，无轮换机制。

### worker 无特权口径

worker 是最小单向面：

- 只能发协议内事件帧（`Auth` / `Heartbeat` / `Fail` / `Scene` / `Done`），
  未知类型在解析层拒绝，连接按死亡处理；
- 连接数受 worker 数上限约束（`--remote-supervise` 的 worker 数旗标），
  每窗口一条连接；
- 不读监督者本机文件：窗口路径清单随 `Assign` 帧携带，worker 不读监督者
  本机临时文件；
- 无文件系统写权限面：worker 不写监督者侧任何文件，失败记录与现场都经
  帧回传。

## 扩展位

本文档定稿持久 server 现状与远程分片安全边界（见上「分片安全边界」一节）。
