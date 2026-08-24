# ROADMAP — “一定程度直接取代 Photoshop” 路线（v0.5.0 之后 · UX 阶段）

> 交接文档：每项都附实现要点与 `file:line` 锚点，供新会话不重读全库即可
> 开工。更新于 2026-08-22（**v0.35.0 已发布 2026-08-21**，tag → `e75f728`，
> 两 exe 字节验证——七项渲染/兼容硬变更清单见发版说明置顶与「当前状态」
> 发布条；**R30 全池 11 项用户拍板开工 2026-08-22**，见「当前状态」首条；
> 此前横幅（2026-08-21 前写，含「发版链未跑」旧句）为
> 历史存档，最早起自 v0.12.0：全量 debug + 协同性审计
> 批次——77 项对抗验证 findings 全修/有据取舍，蒙版行点击=选择恢复、
> recipe.json 统一持久化契约、未保存保护、指针状态机清残留、引擎位图/
> feather/roundness 语义钉死、web/CLI 协同；详见下方「当前状态」首条。
> **v0.13.0 已发布（2026-07-25）**：边车磁盘布局改为中央库+路径键
> （%LOCALAPPDATA%/autoshop/，按照片绝对路径消歧；./out 只留导出成品
> 图；打开时自动迁移旧 ./out 边车），解决同名照片边车相撞 + cwd 相对
> 性。详见「当前状态」首条。
> 前版 v0.11.2（2026-07-17，AI 分析认证修复：`--bare` 永不读 OAuth →
> 三旗标替代 + env_remove(ANTHROPIC_API_KEY) + 透出 stdout JSON 真实
> 错误，src/advisor/claude.rs）；v0.11.1（2026-07-14，中性 cwd 根治
> 信任门）；v0.11.0 → `e3a4096`（perf #3-B 全清）；v0.10.0 →
> `c312a9f`（边车恢复+XMP 导入）；v0.9.0 → `ca6f73e`（GUI i18n）；
> v0.8.1（preview lag root-fix）；v0.8.0 → `1c1ea36`（zoned fit）。
> 反馈驱动阶段——用户试用 → 报障/提需 → 修复/打磨 → 发布）。
>
> **项目收官（2026-08-07）**：用户验收 v0.21.0 的色调模型修复（邮件拼图对比，
> "接收"），并确认 kelvin 重定标（上一封验收邮件）。至此**项目在 v0.21.0 封盘**：
> 已知缺陷清单为空。收官时冻结过两项未验证披露，其一已在收官当日销账：网络
> 恢复后一次真 retouch 现场闭合了**流内尺寸协商全环路**（日志逐行目击：请求
> 柔性尺寸 2048x1360 → API 流内拒绝 → `falling back to 1536x1024` → 流式出图
> → 合成落盘 exit 0）。唯一剩余披露=GUI 运行时验证（用户"绝不弹窗"标令，唯
> 用户可解禁）。后续若重启开发：先读本文件「当前状态」首条 + 项目记忆
> `autoshop-roadmap.md`。
> **（2026-08-08 起开发已重启：用户试用报三问题，第九轮 v0.22.0 见下条。）**
>
> **v0.23.1（2026-08-09，第十一轮 = 队列四项 + 16 路并行 xhigh 全扫收敛 +
> GUI 历史首次真机 E2E）**：队列四项落地（W20 恢复披露 / persist_postponed
> Busy-Io 分型 / ValidatedRecipe / W22 瓦片化引导滤波 3.4GiB→~42MiB）+ 我审
> 抓出的 Prefs 沙箱缺口修复；16 路 Codex gpt-5.6-sol xhigh 镜头并行全 crate
> 扫描 104 发现 → 去重亲证 → 17 项修复落地（variants.json 保存拒删门、UNC
> 词法拒绝、.env PYTHONPATH/权重缓存护栏、Responses store:false、serve POST
> 全令牌+no-store、XMP 曲线越界拒绝/坏色相成对清零……）+ 其余簇有据登记
> 下轮队列；E2E 18 项真进程全过，含 GUI 首次真机启动+截图（用户本轮解禁）。
> 详见「当前状态」首条。
>
> **v0.23.0（2026-08-09，第十轮 = GUI 打磨五项 + 双轨攻击式审阅收敛）**：
> Phase 1 五项（字体嵌入/双主题/排版/按钮/i18n）+ Codex gpt-5.6-sol xhigh 三批
> 全 crate 审计 92 发现 → 36 工作项 35 项落地（W22 余项用户拍板推迟）+ Codex
> 只读全差异 debug 复审 11 发现全闭合。测试 266→312 总，clippy 0，i18n 审计门
> 强制化。详见「当前状态」次条。
>
> **v0.22.0 已发布（2026-08-09，tag `v0.22.0` → `84be2cf`，assets 字节验证
> gui 35840472 / cli 27572908，Latest；E2E 18/18 用户解禁后真进程通过——见
> 「当前状态」次条 E2E 段）**：**第九轮 = 用户反馈三连修 + 蒙版大改**。
> 用户试用报三问题：①变体名"自己会变"；②蒙版要打磨（点名"蒙版本身的调整"，追加
> 增加/排除/交叉）；③AI 整图生成→反推→保存后退出仍提示未保存、「保存并退出」死键。
> 诊断（三路调查工作流 + 8 条根因全部对抗验证 CONFIRMED + Codex 只读复核）：①③同根
> ——**变体带是纯会话态**：三值 kind 被 stash 的 bool 与 pixels.json 的 2 值标志压扁
> （Fitted 无处表示 →「◭ 反推」回来变「▣ 原片」），非活动变体拿自己的 origin 与整
> 照片唯一的 master 记录比对 → generate→fit 后**结构性永脏**，Save-all 只写活动画布
> 且无 Discard 式逃生口 → Close/CancelClose 活锁（"保存并退出"死键）。修法=
> **variants.json 变体带边车**（store 读/写/清三原语 + 共享 publish_json_sidecar +
> recover_orphan_baks/clear_develop 全覆盖；GUI saved_strip 镜像重写背景变体脏判定；
> StashEntry 补三值 kind；Ctrl+S/Save-all 写整条带；成功臂残留复查点名，绝不无声弹回）。
> ②蒙版八件套：components **增加/排除/交叉**（引擎 combined_mask_weight；engine-only
> ——crs MaskBlendMode 无验证参考边车，XMP 只投基形状，面板如实披露）；径向 **angle**
> （引擎旋转 + 旋转手柄 id5 + 滑杆；XMP 投未旋转椭圆=放置 bbox 同款近似）；**眼睛开关**
> enabled（引擎/覆盖/导出门/XMP 四处一致跳过）；**复制蒙版**（栅格 detach 独立生命
> 周期）；列表**活跃 ●**（render::engine_active 与引擎同一条规则）；**笔刷蒙版**（新建
> + 位图栅格编辑：灰度权重缓冲防 8-bit 往返漂移、擦除、羽化/扩展/收缩烘焙、**引导滤波
> 全分辨率精修** worker——预览分辨率 AI 蒙版导出边界发糊的正修）；显示刻度 **0-100
> 对齐 LR**（存储仍 0..1）；XMP 导入**丢弃计数披露**（LR 笔刷/AI/景深蒙版不再无声
> 消失）。验证：clippy 清零、233+30+1 全绿（新增 store 往返/崩溃恢复、GUI 两条回归
> 钉死活锁与 stash kind、引擎组合代数/形态学/引导滤波共 4 测试）；Codex 只读评审 2
> 发现（MaskRefined 需索引+路径双验证防同栅格双引用误指；refine eps 需正下限）已修
> 并复验。详见「当前状态」首条。
>
> **v0.21.0 已发布（2026-08-07）**：**第八轮 = 色调模型正修 + 剩余项清零**。用户令
> "开始下一轮，把所有剩余项清空"。**六轮悬账的色调模型残留正式修掉**：根因=滑杆
> 基函数在原始 x 轴取值、不知道曝光已把顶部基线区间压成零宽 → 新增
> `render::tone_knot_weights`（knot 权重跟随基线自身间隔健康度，`ramp(0,0.01,
> max(相邻基线间隙))`；ev=0 恒等于 1 故逐位不变），被曝光剪裁的区域滑杆自动失权，
> 回归诚实剪裁而非灰色平带；同轮把「单 λ 连坐」也修了（按滑杆迭代收缩负贡献组 +
> 全局 λ 无条件兜底）。270 格实测：**13 格超标/最坏 197 → 6 格/最坏 100**（= ev=0
> 设计自身基线；6 格中 3 格是 65534 差一码即纯白的量化边缘），连坐案例 shadows
> 从被拖到 22.5 → 完整保留 50；栅格测试撤销高曝光 220 豁免档、全域单一 128 上界。
> fit.rs 同接权重（ev 扫描内逐候选加权，模型三处一定义；饱和区天然简并故测试钉
> 「解不劣于真值」而非参数还原，fit 侧权重无可测行为差已如实注记）。E2E 清零段
> 挖出**流内尺寸协商缺失**：`/api/retouch` 真跑时 API 以 SSE error 事件拒绝柔性
> 尺寸 `2048x1360`，而"柔性尺寸被拒→回退枚举尺寸"的协商只装在阻塞 400 臂上——
> 流式模式直接终态失败 → 新增 `streamed_refusal_blames`（带前缀要求：读/解析失败
> 可能已计费，绝不协商重发）。CLI heal(本地)/eval/batch 与 serve
> style-build/download/analyze/heal 全部真跑通过；retouch 客户端经严格 mock 解析
> 器无罪证明（Content-Length 逐字节吻合、九 part 全解析、v0.19.0 边界构造正确、
> 管线对 mock 端到端完成），真端点当日 4/5 次 11 秒传输墙（引擎 3 秒计费安全窗
> 行为正确）。五具名变异（M-T1/M-T2/M-F1/M-G1 + 复验 M-D 系）全灭。详见
> 「当前状态」首条。
>
> **v0.20.0 已发布（2026-08-07）**：**第七轮 = 首次运行时端到端验证**。用户解除
> "只许编译级验证"的限制（"允许运行时、端到端测试"），本轮第一次把真进程、真数据
> 跑起来：CLI 四命令过 61 MP 真 ARW（decode/apply/match/recipe-schema，全程
> `AUTOSHOP_DATA_DIR` 沙箱 + 独立 cwd，用户真实 develop 零触碰）；`autoshop serve`
> 真进程被 20 项 HTTP 断言打满（含 v0.19.0 旗舰修复的实战复验：exiftool 风格
> 纯评分边车 + `/api/xmp` 保存 → 星级/标签逐字节幸存、58 个 crs 属性拼入、库文件
> 未动）；安全守卫四件套全部以**真进程 + mock 监听计数**验证（.env 植入六变量、
> WorkingDir `autoshop.local.json`、设置保存洗白、每个负例配灵敏度正例）。**唯一
> 抓到的产品缺陷是"成功宣称无人核验"**：denoise 边车 exit 0 但不写产物时，CLI
> 打印 `denoised -> 路径` 并 exit 0（产物根本不存在）——而 Opus 对抗复审把初版
> 修复**又击破两次**（陈旧交付物就地冒充本次结果；segment 的 `exists()` 检查对
> 预先认领的 0 字节文件从不生效，是同缺陷的未修同胞而非先例），最终契约收归
> `lib.rs::sidecar_wrote` 三重拒绝（缺失/为空/先于本次运行），denoise+segment
> 同修，五个具名变异全灭，三景真机复验。kelvin 重定标补上了
> v0.19.0 欠的**视觉+数值双验收**：真片 6590K/6610K 对渲染，旧引擎跨缝 R−0.67/
> B−1.11（8-bit 均值）真跳变、新引擎 ≤0.10 连续；四张验收拼图已交用户。色调
> 高曝光残留（contrast −100 @ +1.5EV）在真片高光裁片上**肉眼可见**云纹压平——
> 材料已交用户裁决，仍刻意未修（动色调模型=美学决策）。详见「当前状态」首条。
>
> **第六轮 = v0.19.0 已发布（2026-08-06，`91320ab`；此横幅当时写于发版前，
> "未发版"三字发布后一直没回改——第七轮文档核对时抓获修正）**：**清账 + 再攻自身**。
> 用户令"开工"=清掉第五轮明确记下的五条已知缺陷，并继续用攻击视角审第五轮的修复
> 提交自身。结果**第三次重复同一模式**：本轮最贵的一条 HIGH 又是**本轮自己刚写的
> 修复**——防 multipart 注入的 `choose_boundary` 每轮追加一个 `-` 并重扫全部分片，
> 是攻击者双向可控的 Θ(n²)（1 MiB prompt 实测 7.2 s，而 `RetouchReq::prompt` 无
> 长度上限、body 上限 256 MiB，外推到数小时占着 8 个请求许可之一）。另有两条安全
> HIGH 出在第五轮修复的**边界之外**：ambient 守卫只装在读路径，两个设置写入者
> read-merge-write 会把 cwd 文件里的端点**洗进中央可信文件**（一次"保存设置"即
> 让整道守卫失效）；以及同一攻击经 `.env` 成立且更强（dotenvy 从 cwd 向上搜索，
> `dotenv_override` 连用户真正设置的环境变量都盖）。**本轮明确不改渲染**：色调
> 限幅在非零曝光下的逃生口已实证为真洞（`ev+0.5`/`contrast +100` 塌陷 161/4096，
> `ev+1.0` 为 312），但我的独立复算**否决了子代理"这是上轮修复引入的回归"的判定**
> （加不加限幅器逐位相同，165/312 vs 161/312），且两种候选修法各有坏区（去掉逃生
> 口→λ 归零五滑杆全死；按滑杆分别限幅→正曝光与"误伤其他滑杆"都解决但负曝光更差，
> −2EV 组合 98→201）。根因是节点间距约束只是代理量，Fritsch–Carlson 仍可能压平
> 节点已分开的跨段——真正的修法在曲线空间，且**无 GUI 无法做视觉验收**，故记账
> 下轮做。详见「当前状态」首条。
>
> **v0.18.0 已发布（2026-08-06）**：**五代理分区攻击审计——两个 HIGH 出在"上一轮的
> 修复"自身**。本轮把整棵树拆给 5 个独立子代理（上轮修复提交、gui.rs、渲染/反推
> 数值核、store/config/CLI、advisor 网络层），逐条对码复核后修 9 条。要害两条都是
> 第四轮**修复动作自己引入的**：①XMP 扫描器"O(n) 重写"仍是 Θ(n²)，只是换了形状——
> 嵌套修好了（2.8MB 55.97s→51ms）但注释/PI 形状**倒退了 16.5 万倍**（640KB 51µs→
> 8.47s，实测）；②`api_xmp` 新加的 clamp 会把"零面积裁剪"这类请求**夹成中性配方**，
> 从而落进删除分支——同一个请求在加 clamp 之前走的是保存路径，即**新引入的数据丢失**。
> 另有渲染引擎 HIGH：普通滑杆值会**整段抹平色调**（`whites -50` 使高光十档从 411 个
> 16-bit 码降到 75；`highlights +60` 把 18% 动态范围压成纯白），四轮测试没抓到是因为
> 断言只查"单调 + 端点"，而**平带既单调又保端点**。详见「当前状态」首条。
>
> 前版 **v0.17.0（2026-08-06）**：**攻击视角安全批**。独立子代理以攻击者立场
> 审全仓，8 条发现经逐条对码复核**全部属实**；两条 HIGH 是前三轮自审都没看见的：
> 跨域门把无端口的 `Origin: http://localhost` 当通配（loopback:80 上的页面可改
> AI 端点，下次 Analyze 即泄露 API key），以及请求许可在 handler panic 时永久
> 泄漏（8 次 panic 后服务器彻底停止应答）。另修 XMP 扫描器 Θ(k²) + 注释误判、
> 唯一持久化路由不 clamp、XMP merge 失败静默丢 Lightroom 属性。详见「当前状态」首条。
>
> 前版 **v0.16.1（2026-08-06）**：单通道 debug + 死代码/重复清理批。
> 要害修复=XMP 读取端穿透嵌套 `<crs:Look>`（顶层缺键时把 Adobe 配置文件
> 烘焙的参数当成用户滑杆导入，探针实证 clarity 0→50 / vibrance 0→35，还会
> 学走 Look 的色调曲线）→ 新增 `xmp::crs_own_scope`，四个整档读者全部改走
> scope；顺带 `apply_wb` 行并行（v0.11.0 rayon 扫荡唯一漏网的逐像素段，
> 逐位不变）、一条 i18n 死翻译、一条瞎断言、七处重复的等价合并、一处过期
> 的 `#[allow(dead_code)]`。详见「当前状态」首条。
> 发布后又跑了两轮 debug（第二轮 `5aa9d28`、第三轮本条）：**生产行为未变**——
> 只补测试、订正两条假注释、消掉一处两端手搓的契约重复，并把 README /
> ARCHITECTURE 与代码对齐（补上此前完全缺席的桌面 GUI 与 `match` 反推）。
> 故不发新版，已发布的 v0.16.1 二进制不受影响。

## 标签定义（LR 差距批次 / 素材 / 分叉编号 —— 2026-08-18 入库）

> 上面第二十二至二十四轮段（`M0`、`B2-B5`）与下面「当前状态」多处引用这些标签，
> 而定义此前**只在不入库的计划档**里（`feedback17-final-plan-r22-r24.md`、
> `feedback17-xcheck-consensus.md`、`r25-design.md`）——接手的人无从查证，这是台账
> 的真实漏洞。定义在此入库；计划档仍是工单级细节的出处，**含义以本节为准**。

### 基座与批次（源：R24-5「LR 缺口分批」）

| 标签 | 定义 | 状态 |
|---|---|---|
| **M0** | 五档控件 tier 注册表（`Rendered` / `CarriedOnly` / `PassThrough` / `RenderedNotExported` / `DerivedWriteOnly`，继承 V2_PLAN §7）+ 两条包含断言（GUI 可改 ⊆ 引擎渲染 ∪ CarriedOnly 白名单；AI 可设同）+ 三方向披露（导出蒙版具名 / 导出全局 / 导入全局走补集） | ✅ R24 v0.30.0 |
| **M8** | 投递根统一：`./out` 由散落常量升为一等设置 `config::delivery_root`（Destination 信任），库侧 pipeline / serve / style 各消费点同走一个漏斗 | ✅ R24 v0.30.0 |
| **B2** | 全局 `crs:Texture` + 效果面板：Texture **读写必须同一次落地**（只读不写＝键不进 `owned_attr_keys`，merge 不剥离，我方值与文档原值并存；只写不读＝导入的 LR 值不生效）；裁剪后暗角六键 + Grain 三键先走 CarriedOnly；「暗角」标签同名冲突改名 | ✅ R25 v0.31.0（`1ddf53e`） |
| **B3** | Detail 子控件（锐化 3 + 亮度降噪 2 + 彩色降噪 3）、手动色差 `ca_r`/`ca_b`（渲染）+ `AutoLateralCA`（携带）、Defringe 六键 | ✅ R25 v0.31.0（`98b4c65`；Defringe 走甲案＝CarriedOnly 终态） |
| **B4** | Transform/Upright 八键 + Camera Calibration 八键走 `Tier::PassThrough`（**具名键集**，不是「一切未知」——merge 的剥离宇宙是静态清单，自由 map 会与实际写出的键失配）+ 渲染分叉披露 `global_render_gaps` | ✅ R25 v0.31.0（`3ae7df7`；PassThrough 首个真载荷） |
| **B5** | 蒙版几何互通，三臂：**A＝导入**（LR 蒙版解锁，与 `INERT_LOCAL` 硬拒同根因一次修完）✅ R25 `4eb54aa`；**B1＝写回** `crs:Midpoint`/`crs:Version` + 旋转损失点名角度 ✅ R25 `a98a82f`；**B2＝`crs:Angle` 双向映射** ⏸ E1-verdict2 扩样（冻结预注册、来源门控 10 照）**未复现** 5 照佐证：4/10 顺时针、Stouffer Z=−1.431、3 逆向照 \|z\|>1 ⇒ 冻结判级 **evidence WEAKENED**（主分析 n=2 单独仍 SUPPORTED Z=+1.695，分歧照预注册并报）；引擎自身旋向（引擎渲染实测）与三家独立重实现不受影响，码内本就未映射零回滚；**唯一收口路径=用户 LR 已知角度实验** —— **该实验 2026-08-18/19 已做，三臂全合于 R26 v0.32.0 ✅**：12 张受控导出+ 像素测量定下角点解码 / `k=1.032` 坐标仿射 / 落笔两端点 / 极性真值表（证据档案 ~/.claude/plans/r25-materials/lr-experiment/）。**← 2026-08-19 勘误（R27 Batch-8/10）**：这四项里 `k=1.032` 与落笔两端点已被后续实验推翻——k 实为**每帧**镜头档案畸变而非常数，用户拍板 `LR_MASK_FRAME_SCALE 1.032→1.0`（`xmp.rs:161`）；`ramp(1−f, 1+f/2)` 两端皆被直测证伪（d_out 在 f≥50 饱和 ≈1.41），码内登记不动。角点解码与极性真值表不受影响，详见「当前状态」v0.33.0 条 |
| **SF4** | 「是否接受 `CarriedOnly` 这个中间态」（字段可读可写可编辑、本机预览不渲染、进了 Lightroom 才生效＝公开承认预览与 LR 不一致）三选一：**A** 全盘接受，B2-B4 全部可落码；**B** 不接受，那批全退回 PassThrough + 披露，App 只做自己渲染得了的；**C** 接受但**限白名单**——只对 Adobe 独有、我方短期实现不了的算子开口。**用户 2026-08-18 拍板 = C（默认档）**，24 个全局成员逐条带理由钉在 `CARRIED_ONLY_GLOBAL` | ✅ R25 定稿 |

### 素材请求（用户侧动作，非代码项）

| 标签 | 内容 | 状态 |
|---|---|---|
| **M-A** | 一组新的「反推瞎搞」复现对（RAW + 参考成片），给联合梯子建真实基线 | ✅ R24 自采六对真同帧 |
| **M-B** | 一次 LR 采样实验的 sidecar 族：①径向旋到已知角度 ②径向组件组合 ③含局部点曲线的蒙版 ④AI/画笔蒙版 | ✅ **全部交齐**。R24 自采 160 份取证覆盖③④与阴性结果；**①已知角度 ✅ 2026-08-18 十二张受控导出已交，已在 R26 v0.32.0 全部消费**；~~Defringe 非零仍欠~~ **← 2026-08-19 勘误：Defringe 非零同在 2026-08-18 那批里已交**（`_DSC9597.xmp: crs:DefringePurpleAmount="10"`、`DefringePurpleHueLo="19"`、`DefringePurpleHueHi="49"`；同批另七份全带静息块 `0/30/70` + `0/40/60`，反过来独立确证 R25 选的非零默认）。该行原是一次部分编辑的残留，不是判断 |
| **M-C** | `f944ef3` 那 147 张 RAW+xmp 对照集是否还在、路径确认 | ✅ R25 全量重跑（数字见 v0.31.0 条）→ **R27 第三次尝试落地成新基线（2026-08-19）**：HEAD `f43dd85`、中转端点（gpt-5.6-sol）+ `--jobs 3` 并行、147/147 全 done、**双流零回退**、约 38 分钟；`~/.claude/plans/r27-materials/mc-eval-147-r27.txt` 自此为唯一参照，R25 降为历史。前两次尝试在案：#1 死于 83/147 commit 内存墙（Batch-7 已根治）、#2 有 2 条中转上游 `stream_read_error` 回退按判据废稿。头条：**gap 15.5%；蒙版拒收 2.35→0.05 张/照（Batch-4 导入根修实证）；whites 偏置 +22.55→+16.20、blacks −14.18→−10.87** → **v0.34.0 重基线接替（2026-08-20，B1a 重试+续传首个实战）**：attempt-1/2 各废于一条中转流错（`response.failed`，判据执行）；attempt-3 单进程 147 全 done 但 **5 回退**（4×中转 http 524 计费不明类＋1 畸形 JSON）+ **1 硬败**（Claude 529 过载）＝报告混入启发式行弃用；**续跑 141 行免费载入＋6 张补买零回退**（提源行 `147 rows: 141 loaded, 6 measured, 0 fallback`）→ `~/.claude/plans/r28-materials/mc-eval-147-v0340-resume1.txt` 为现行唯一基线（同目录 attempt-3 全量转录留档），R27 降为历史。头条：**gap 16.0%**（vs R27 15.5%——R28 5b 四条 `color_grade.*_hue` 行口径改两侧过阈自报跨版不可比，其余行可比）；whites +16.12 / blacks −10.60（与 R27 同量级）；蒙版拒收 0.05 张/照持平；AI 蒙版 1.99 vs 用户 2.30 张/照。**登记 R29 候选**：524/529 类非流死亡瞬态传输失败是否纳入重试（本次漏斗 6/147≈4%，524 属计费不明须拍板；已落 `74ffa25`）；W_EMB 两臂标定仍欠第二臂（已于 v0.35.0 判分窗闭合，见下） → **v0.35.0 重基线接替（2026-08-21，拍板八，发布位 exe）**：`~/.claude/plans/r29-materials/mc-eval-147-v0350-resume1.txt` 为现行唯一基线（`147 rows: 145 loaded, 2 measured, 0 fallback`），v0.34.0 降为历史。头条：**gap 17.7%**（vs 16.0%——**⚠ 差值=verifier 效应+采样噪声，与 R29 渲染硬变更无关**（2026-08-22 勘误：eval 不渲染像素，proposer 提示词/量表跨版逐字同=`openai.rs`/`catalogue.rs` 零 diff 亲证；verifier 从 oauth/opus 换成中转 gpt-5.6-sol 系 opus 经继承中转 env 产 16% 畸形 JSON 被迫切换，verify→revise 环直接改提案数值），不构成渲染回归证据）；whites +14.85 / blacks −10.91；蒙版拒收 0.05 张/照持平；AI 蒙版 1.99 vs 2.30 持平。W_EMB 标定同窗闭合（离线 leave-one-out，settings 目标平坦 CI 跨零 → **默认 2.0 不动**；细账见「当前状态」判分条 + V2_PLAN §7-8）。判读细读归用户 → **同版本重复跑伴随数（2026-08-22，拍板②付费，非接替）：gap 16.3%**（同 exe/库/模型全新 state 两续传 0 fallback，`mc-eval-147-v0350-repeat1-resume2.txt`）＝同版本重采样摆动 1.4pt 与跨版 +1.7pt 同量级，「非回归」获直接实验证据；基线仍为 17.7% 那份不变，公开引用须双数并报（细账见「当前状态」重复跑条） |
| **M-D** | #2 蒙版精修报错的 toast 原文（分流哪条臂） | ✅ R24 两臂都修 + 实跑否证 |

## 当前状态（已完成，勿重做）

- **🎯 D2 线性 H2 修复批已落地（2026-08-24，feat 见本条提交；用户拍板
  「全改」两路）：线性蒙版帧法与径向拆分**。`MaskFrame` 三态=
  WarpedDownstream（径向 `MaskUnwarp::at` = m_lr⁻¹∘T_engine **字节未
  动**；线性新 `engine_at` 仅 T_engine）/ LinearHandlesToRaw（无下游几何
  但有相机图：线性两手柄一次过 D_fwd 后重建直线，`Cow` 一次准备、像素
  环不调图）/ AsRendered；`mask_weight_in` 由 `Radial|Linear` 共用谓词改
  显式双臂（旧谓词降 cfg(test)）。**新字段 `LensProfile.linear_handle_
  warp`**（enable=0 时 `fresh_lens_profile_for_sidecar` 把解得图从
  mask_warp 迁入=径向保持存储恒等、线性保留手柄图；clamp 钉「仅
  DisabledInSidecar 且 ≥2 结」一态一义；**v1.0.0 窗内第二处 schema 硬前
  向断裂**，旧读器拒读、旧配方默认空=旧恒等行为；替代方案（复用非空
  mask_warp/开图时重解/瞬态旗）均有据拒）。GUI 两处 range 参考路径改
  `without_downstream`（径向留存储、线性得手柄律）。**精度披露**写在
  render.rs 帧图节：非 1px 闭合（ON 9.748/7.025/6.336、OFF 12.449/9.943/
  4.979px RMS），拟合级各向异性**不实现**。门主审亲跑：**871(862+9i)/
  14/132/2+2 全绿、clippy -D warnings 0**，集差恰 +5 具名（pipeline 禁
  用边车只留线性图/线性 ON 落存储校正帧线 <0.1px+栅格 <0.3px/OFF 三墙
  线手柄输运 <0.01px+位移 −29.9/+28.7/−30.7±1.5/OFF 直线性 sag<0.5px 且
  H1 对照 >1.5px/径向禁用+留图仍存储 <1px），diff 文本零 `#[test]` 删
  除；**三变异亲手红**（线性回径向复合采样器/禁用边车清图/只输运
  Zero 手柄）字节级还原。⚠渲染硬变更：含线性蒙版+相机档案配方双臂全
  变；无发布版曾载 706ac84 线性行为=义务并入 v1.0.0。登记：`linear_
  handle_unwarp_norm` 与径向原语 40 行同构重复（测试以 <0.01px 钉同
  图，v1.0.0 后可去重）；V2_PLAN/ARCHITECTURE 无线性帧法段落（W4 补）。
- **🎯 D2 线性机理批=H1 逐点证伪+H2 手柄输运保直线定向（2026-08-24；
  报告 `mw-exp/d2lin/d2-linmech-report.md`，归档 r30-materials）**。六
  条密轮廓（546/546/392 点×双臂，与判决批独立窗口互证 0.4px 内）。
  **H1（校正后帧整线逐点前向弯翘）决定性证伪**：预测凹陷
  −22.8/+24.4/−10.8px vs 实测 +0.6/−2.9/−0.5px——三线全反号、幅度差
  8.5-41×（主审结值手算独立复现预测域 2820..2864 与凹陷量级）。**H2
  =手柄输运保直线为最佳零参数拓扑**：线性坐标存在校正后帧；校正开=手
  柄直用重建直线；校正关=两手柄过 D_fwd 后重建**直线**（非逐点）。零
  参数 B 臂 RMS 12.4/9.9/5.0px、成对 A−B 4.3/4.7/2.8px；**非 1px 闭
  合**——余量=线级共模（A 臂常量偏置 −9.4/+6.5/+6.1px；二阶候选=各向
  异性长宽比 sx1.00376/sy0.99634 拟合级、或光度交叉偏置，未坐实）。
  **引擎建议**（批+主审同意）：现行 706ac84 接线=最差（A 臂 RMS
  21.5-31.5px）；ON 路=仅 lens_ungeom（落存储校正帧直线，9.7/7.0/6.3
  px）、OFF 路=手柄输运+保直线（12.4/9.9/5.0px，需线性专用帧图事实—
  —现行 LensProfileEnable=0 清 mask_warp 不能复用）；`Radial|Linear`
  共用谓词=错误抽象边界，拆分且径向零改动。**1px 闭合素材**（可选登
  记）：孤立单蒙版网格 3 竖+3 横 ON/OFF 孪生=分离轴尺度 vs 光度偏置；
  本裁决不需要。候选二阶各向异性对 fill/RR/DCO/舍入全量化排除
  （≤0.23px/0.5px 级）。接线终拍板待用户。
- **🎯 ⑦ 线性 armB 判决=预注册闸决定性失败（1/9），线性蒙版帧语义
  与径向根本不同=一级新发现（2026-08-24；判决报告
  `mw-exp/d2lin/d2-linearverdict-report.md`，素材 v2 重建经过在同目录
  d2-linear-report.md v2 附录）**。素材线：v1 三块 −4EV 内向渐变叠满全
  帧压穿 8-bit（全黑判废，根因=任务书缺轮 5 已立的导出域叠层地板检
  查）；v2 主审亲手重建（方向外翻+幅度 −2EV、中线与九点预注册逐字保
  留、孪生/读回/地板五路亲验）后台阶 87-90 codes 全可测。**判决三事
  实**（主审 L1/L3 双臂四项独立抽核 1.5-3.2px 内吻合采信）：①
  **armB（校正关）线性蒙版不在存储坐标**——中线外扩 −41.1/+37.7/
  −25.8px（约图心离心向、轮廓微弯），vs 径向蒙版 41 向量 B 臂共模仅
  0.6px 级；② **轴上 A−B 与 m_lr 点输运惊人吻合**（三个 50% 探针误差
  +1.20/−0.37/+1.17px，L2/50 过闸）但**离轴 A−B 近似沿线刚性平移**
  （观测 ~31-41px 恒定 vs 预测 6.6-18.5px，误差 12-34px）=点输运律对
  线性证伪；③ **保真算术：LR 的 A 臂距存储 4.4-22.1px（8/9 探针更
  近）、距 m_lr 输运均值 23.9px ⇒ 706ac84 的线性接线使 A 臂保真恶化
  ~2.7×**。径向 41/41 定案不受影响。armB 外扩非 m_lr⁻¹（幅度比
  1.32/1.24/0.83 不齐）、非单一径向律（2.14%@0.336/1.82%@0.363/
  1.56%@0.288 非单调）=机理开放，两张 v2 导出在手可续分析。**待拍
  板**：撤线性接线（回存储渲染）继续 v1.0.0 vs 先追线性机理。批纪
  律：估计器固定双臂同跑、armB 未扣共模、闸未动、仓库树净 706ac84。
- **🎯 D2 修复批已落地（2026-08-24，feat 见本条提交）：LR 蒙版几何零参
  数模型实装**。Sony 畸变结原生域 (i+1)/16 在 lensmeta 边界重采样
  2048 结（**限蒙版求解**；渲染样条经配准闸有据保持）+ mask_warp 图心
  =全帧中心−DCO（`LensProfile.mask_warp_center` 新字段=schema 硬前向断
  裂，v1.0.0 窗口）+ 输出表 16→64 结（clamp `truncate(32)` 会砍 64 结
  的旧洞同笔堵）+ `MaskUnwarp` 改双图恰一次组合 m_lr⁻¹∘T_engine（数学
  论证=集合 {p:T_engine(p)∈m_lr(shape)} 导出恰落 m_lr(stored)）。门主
  审亲跑：**857(9i)/14/132/2+2 全绿**（集差恰 +5 具名：lensmeta 重采样
  契约/41 向量零参夹具/图心/组合恰一次/实栅格化 wired 落点）、clippy
  0、**三变异亲手红→绿**（落位回退/图心回退/16 结回退=G3 1.0095px 恰
  破 1px 闸）。**渲染侧配准闸**：仅换结候选使墙面 RMS 2.83→3.62 被拒、
  渲染字节不变（SHA D6B721…/BE4950…）——但该候选未含图心移位、左好右
  坏签名与蒙版侧同构⇒**登记跟进：图心齐备的渲染候选未测**。精度披露：
  点 ≤1px/洁净膨胀 ≤0.35pp/R1≈0.5pp/R2 大蒙版 ≈1.2pp 超额=独立开口。
  105mm B3 与 wired 0.09-0.30px 幸存论证在批报告 §6（归档
  `~/.claude/plans/r30-materials/task-d2-fix-report.md`）。**⚠v1.0.0 义
  务**：含径向/线性蒙版+相机元数据镜头档案的配方渲染全变+schema 前向
  断裂（旧 exe 拒读带 mask_warp_center 的新 recipe）。台账拆分第二批：
  轮 7a 及更早 12 条逐字迁入 docs/ROADMAP-archive.md（主文件保近五条）。
- **🎯 D2 轮 8c 机理零参数定案：图心=全帧中心−DCO、函数=引擎反演唯一
  改 knot 落位 (i+1)/16 ⇒ 双场景 41/41 点向量 ≤1px 零自由参数
  （2026-08-24；主审亲跑收尾，Codex 中转两死；报告
  `~/.claude/plans/r30-materials/d2-delta-report.md`）**。**图心第一方
  证实**：decode.rs:1211-1213 记载 DCO(32,20)+DefaultCropSize 9504×6336
  嵌于 **9600×6376 全帧** ⇒ 存储系图心 (4768,3168)，距自由拟合 0.4px、
  自由重拟合不再改进=元数据图心即最优。**推导误差=raw knots 半径落位**：
  移植引擎反演（复现 1.6e-7/2.5e-7、fill 逐位同）后 80 候选零参数格扫
  描，(i+1)/16（末结恰在画幅角 r=1）胜出——引擎 (i+0.5)/15（末结 1.033
  出角）同图心仅 22/41；**胜者+密化输出表（16→64+结）=wall 20/20
  （RMS 0.568/max 0.916）+DSC 21/21（RMS 0.243/max 0.683）全闭**；
  f(0.333) 校验 −0.01643/−0.01529 vs 提取 −0.01683/−0.01540。**尺寸律
  =有界残差**：boundary_mean R1 双集首次 PASS（−0.492/−0.471）、
  boundary_geo DSC 15/17，洁净格 RMS 0.13-0.17pp 但 3-4 格 0.2-0.35pp
  超 0.18 闸；**R2 大蒙版超额 −1.0..−1.2pp=独立小机制**（同蒙版点律
  0.3px 闭合；候选=LR 蒙版栅格化）。**修复设计输入就位**（图心元数据
  式+knot 落位改正限 Sony CameraMetadata 源+表密化+2615-2700 整臂反推
  义务清单在报告 §5）；零修复授权维持待拍板。**⚠中转事故**：8c 重试批
  陷「Testing direct functions.exec call」退化循环烧 2,994,622 tokens
  后二次 403 断供（退款证据=request id 67c9827c-…-812281405500）——两
  拍板已上呈：①机理是否就此定案进修复批 ②中转退款/充值/模型通道。
## 关键架构事实（新会话必读）

> gui.rs 已于第十二轮拆为 `src/bin/gui/*` 模块树（app/actions/canvas/
> workers/export/persist/masks/model/util/i18n/theme/panels/*）。下文与
> ①-⑤ 历史条目里的 "gui.rs" 锚点指其对应模块。

- 所有图上交互经 `ViewXform`（屏幕↔全幅归一化，gui/model.rs）；工具互斥
  分发在 `after_view`（gui/canvas.rs；crop > placing > wb_pick >
  range_pick > clone > paint > box-select）。
- **EXIF 方向在链条最前端**（55e7e07 起结构就位，**v0.30.0 起对 ARW 才真正
  生效**）：引擎 `orient_f32` 在 develop 之前转正 f32 缓冲，decode 端
  `preview_only`/`decode_raw` 用同一 `render::oriented`（pub(crate)）转正内嵌
  预览——GUI 显示帧 == 引擎 original 帧。rawler 的 ARW 内嵌预览本身
  **不带**转正（crate 源码实证）。
  → **方向值的唯一来源 = `decode::raw_orientation_of`（EXIF IFD0 tag 0x0112）**：
  rawler 0.7.2 在 `rawimage.rs:389/478` 把 `RawImage.orientation` **硬写死为
  `Normal`**（`//cam.orientation, // TODO fixme`），DNG/QTK 之外全部解码器如此——
  所以 v0.29.x 以前竖拍 ARW 在显示/显影/导出全链都是横的。**五个**消费点
  （render.rs 渲染钩、decode.rs 的 Meta 尺寸 + 预览转正、`camera_rendition`、
  `frame_size`（v0.32.0 起）、`pipeline::migrate_recipe_coord_frame` 按路径
  孪生——2026-08-20 深检由三勘正为五，ARCHITECTURE 同句早已改）
  均改读该访问器；缺 tag 回 `Normal`，rawler 自己的 `from_tiff` 回 `Unknown`，
  二者在像素/坐标/尺寸三条链上均为 no-op（断言在
  `unknown_and_normal_are_the_same_no_op`）。GUI 缩略图磁盘缓存盐 v2→v3
  （gui/util.rs），否则旧缓存继续端出歪图。
- `develop_preview`（render.rs）跑 `apply_recipe_wb` + `apply_develop`；
  **不应用裁剪**（GUI 用 uv 窗显示、导出端真裁）。**几何链**由 GUI `redevelop`
  在 develop_preview 之后依次调引擎 `apply_lens_distortion`（C2 畸变）→
  `rotate_straighten`（拉直）完成（导出路径同函数、同顺序）。
- **坐标帧代 `EditRecipe.coord_era`（v0.30.0 新字段）**：0 = v0.29.x 及以前写的
  配方，其 crop/masks 存在**传感器帧**（1 = EXIF 显示帧）。载入时由
  `pipeline::migrate_recipe_coord_frame` 一次性纯旋转双射迁移（`render::
  orient_point` = `oriented` 像素变换的坐标孪生）。**故意不复用 `version`**：
  `version` 是基调曲线的 provenance 且被有意地在配方间**移植**（paste_recipe_for /
  produce_recipe / photo_calibration / 退出保存重盖），把坐标帧搂进同一个整数会让
  “目标照片的 era-2”盖到已是显示帧的几何上→下次载入会**再转一次**。
  新字段对旧 exe 前向不兼容（`deny_unknown_fields`，同 color_gains/role/hue
  先例，已登记在发版说明）。迁移**只挂在读文件的载入点**（GUI 开图 /
  变体条 / 版本快照 / 批量导出 / api_recipe / CLI apply）；HTTP 请求体与 AI 返回
  的配方在边界上直接盖为当代帧（`serve::live_frame_recipe` / `advisor::openai`）。
  **栅格蒙版（手绘/AI 分割）是图片文件，不迁移，改为向用户披露**。
- **坐标空间约定（④起，C2 扩展）**：original →（畸变校正）→ corrected →
  （旋转+内接裁剪）→ view；`recipe.crop` 存 view 空间；masks/画笔/吸管/
  region 存 original 空间——gui/util.rs `view_norm_to_orig /
  orig_norm_to_view / geom_to_view`（三者带 `dist` 参数，来源 `geom_ctx`）
  在数据边界换算，共用引擎 `inscribed_dims / distort_norm /
  undistort_norm`，全零恒等。完整合约见 render.rs "Manual lens
  distortion" 注释块。
- tone 模型单一事实来源：`render::TONE_KNOTS_X / tone_slider_basis /
  tone_exposure_curve`（pub(crate)，fit.rs 逆着它解）；曲线采样单一事实来源
  `render::curve_lut`（pub，GUI 曲线编辑器直接画它）。
- `recipe.masks` 是 AI 与手动共用的同一列表；引擎 `apply_masks` 实时渲染
  **WB(temp/tint)+color_gains → tone → saturation → NR**（#2-B 起；WB 镜像
  全局 `wb_gains` 模型、mired 映射 `local_temp_to_kelvin`；`color_gains`
  是分区反推的重着色增益，引擎专用），clarity/dehaze/texture 仍仅进 XMP
  （GUI 已如实分组：Temp/Tint 移入实时区）。**（R22 起已上引擎**——此句
  记录的是 #2-B 当时状态；现行通道链见下条 R22-4）
- **R22-4（#15a/#10B）蒙版 clarity/dehaze/texture 上引擎**：`apply_masks`
  现行通道顺序 = **dehaze → 融合 WB+tone+sat(+hue) → clarity → texture →
  sharpness（R23-1b 起，±100 有符号）→ NR**（2026-08-20 深检补 sharpness/hue
  两级），每条各自 `!= 0.0` 门控（此前「只调这三项」的蒙版三重落空：不渲染、
  `engine_active` 判不活、栅格预算加载器连位图都不载）。dehaze 复用全局
  同一模型（`apply_dehaze` 拆出 `dehaze_airlight` + `dehaze_px`，拆分前后
  golden 位级一致），airlight 每帧只估一次且取自全局显影后的画面 ⇒ 蒙版
  叠放顺序不改变雾模型；clarity = `unsharp_luma_weighted`、texture 自 R28 起 = `render::texture_pass`
  （正半支即 unsharp、负半支带限 fine−coarse——2026-08-20 深检补注；把权重
  乘到亮度差上，与「整幅滤波再按权混合」严格等价，故仍只需两个 f32 平面而
  非 RGB 副本，61MP 省 ~732MB），clarity 半径同全局（0.02·短边，地板 8px、
  midtone 加权），texture = 0.005·短边地板 2px、无 midtone 加权且**是我们
  自己的标定**（引擎无全局 Texture 可对齐、Adobe 模型未公开，同
  `manual_vignette_lut` 的诚实口径）。**两处与全局链的残差如实登记**（局部
  WB/tone/sat 是一次融合混合，拆开会改变所有既有部分权重蒙版的输出）：
  局部 Temp/Tint 落在局部 dehaze 之后；局部 saturation 落在 clarity/texture
  之前。`engine_active` 同批加三项，两个消费点（GUI ● 与栅格预算 filter）
  行为自动同变。**行为变更**：既有带这三项的旧 develop 会重渲染出新观感
  （用户已批），judge 视觉评审看到的像素随之变化 ⇒ 分数基线移动（R23
  强度轴验收前无需重标）。GUI「More (XMP/Lightroom only)」折叠头**已在包 4
  删除**：三根滑杆并回主列表，选中蒙版的调整按 Lightroom 三组分组
  （Tone/Detail/Color 弱小标题）＋「有蒙版未选中」补提示＋Temp/Tint shift
  语义 tooltip（等效开尔文取自 `render::local_temp_to_kelvin` 本体，GUI 不
  重抄数字）＋`color_gains` 弱提示与「↺ 清除」。
- **M6a 导出侧有损披露（包 4）**：写侧 `masks_xml` 边发 XML 边产出
  `Vec<MaskLoss>{name, reason}`（`Bitmap`/`Disabled` 跳过 ＋
  `ComponentsFlattened`/`Rotation`/`Recolour` 降级；**每蒙版只出一条跳过
  判据**，静音的位图蒙版不双计），`xmp::mask_export_losses` 是唯一来源、
  `describe_mask_losses` 是唯一英文文案；清单经 `pipeline::write_xmp` /
  `write_xmp_at` 返回值第三位穿到每个落点（GUI 保存与 Analyze 落点渲染本地
  化 toast＋状态行、serve 两条响应各按既有 `warning`/`\n⚠` 惯例并入、CLI 走
  `write_xmp_doc` 一处 stderr）。此前 `unsupported_corrections` 四个调用点
  全在 import 方向，export 方向零披露。批量粘贴/Save-all/分区反推三处刻意
  只吃 stderr（各自的既有文案是「失败」或「已由 rationale 说过」，混入会
  误标）。
- 分区反推 `fit_zoned.rs`：`fit_recipe_zoned`（CLI `match --zoned` /
  GUI `zoned_fit` Pref）= 全局 fit → 天空分割×2 → 天空+地景（同栅格反相）
  双分区 → 每区 zone_err 矩裁判（帧全局 look_err 只作 ±0.02 漂移保险——
  帧级指标会否决正确分区重绘，实测记录在 ZONE_ACCEPT_RATIO 注释）＋区内
  luma-CDF 色调求解（源区 IQR<0.05 退化守卫）。任何失败优雅回退全局 fit。
- 源照片库只读（`pipeline::guard_readonly`）；输出走 `config::delivery_root()`
  （R24 M8 起为一等设置：settings `out_dir` > `AUTOSHOP_OUT_DIR` > 默认
  `./out`；guard 把配置根与字面 `./out` 都算输出区——见 ARCHITECTURE §4.10。
  原文「输出一律 ./out」滞后于 R24，2026-08-20 修正）。

## 完成每项后的例行动作

1. `cargo clippy --features gui --all-targets`（零警告）+ `cargo test --lib`
   + release build + GUI 启动烟雾。
2. 密钥扫描（`sk-[A-Za-z0-9]{20,}|OPENAI_API_KEY=|ANTHROPIC_API_KEY=`）后
   提交（结尾 `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`），
   用户说 push 才推、说发布才发 release。
3. 攒够一批（如 ①②③）可提议发 v0.3.0。
