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

- **D2 轮 6a 函数形扫描=20 候选全灭但战线收窄至 boundary_mean 一线
  之差+墙面素材定案（2026-08-23，零素材桌面批；报告
  `~/.claude/plans/r30-materials/d2-sizefunc-report.md`）**。闸先行
  （净格 0.18pp/R1 0.5pp/R2 仅参考/edge_S 永不入拟合），14 零参数
  +6 单参数候选逐格残差全表：**最优 `boundary_mean`**（边界弧长
  (T+R)/2 均值）fit max 0.176pp 过、净嵌套 max 0.246pp（centre_L 超
  闸 0.066pp）、R1 −0.534pp（超闸 0.034pp）——一线之差非大溃败；纯
  切向/纯径向两族 ~2.5pp 灾难失配=**机理等权混合 T/R 实证**；单参数
  混合全部塌缩回基函数或不救 R1。**系统性同号残差**：centre_L/R1/R2
  三失配全为「实测>预测」=大蒙版一律比任何 camera 图平均更膨胀（下
  一分析线索：LR 内部档案≠CameraMetadata 或非线性尺寸项）。复制格
  离散 0.026pp RMS=轮 3/轮 5 复现再证。三族预测已锁定（boundary_mean/
  conic/field 逐格）+分离几何点名=平坦场景 **centre-L**（0.161/
  0.354pp 分离）+edge-S 噪声对照。**墙面素材定案（用户提议亲验采
  纳）**：R29 B6 墙面成像（me6×20/me7×9/exp_B 家族字节全同 SHA
  FE5745…1E26；ILCE-7RM4A+FE24-105 @24mm 同 DSC08276 配置、四位置
  亮度 120-138/255 std<5.5 平坦、HasCrop=False⇒导出=存储帧 9504×
  6336 零裁剪换算）——零新拍。**轮 6b 墙面孪生构建批在飞** d2w/
  （d2n 同几何 3 中心×S/M/L、−0.375 曝光带渲染探针义务、三族锁定预
  测+中心律预注册入报告）；建成待用户 LR 导两张 9504×6336。
  开（2026-08-23，用户 d2n 双导出当日交付；报告
  `~/.claude/plans/r30-materials/d2-nested-verdict-report.md`（正本
  d2n/）；18 轮廓全 240/240、armB RMS 0.780px 无几何位移、M 三格复
  现轮 3 ≤0.148px/≤0.04pp=跨轮稳定性内控过）**。**测试 A 中心律
  CONFIRMED**：三位置中心位移尺寸无关（S→L 漂移 ≤0.364px）且中标中
  心点直传预测（camera 前向；新鲜引擎逆差 ≤0.09px），死掉的拟合心
  律在 L 格被 13.2/16.4px 拒绝——蒙版中心=预校正帧锚定+点直传，就此
  定案（角上 ~2.8px 绝对校准残差留档）。**测试 B 等距性 FAILED（一
  格）**：8/9 过 ≤0.3pp 闸，A/edge_S 裂 0.855pp——但其 B 孪生（零几
  何）也裂 0.480pp+拟合残差偏高=指向暗区小椭圆探测噪声非真各向异
  性；批拒绝豁免自家预注册规则（正确纪律）。**测试 C 尺寸律无胜
  者**：圆锥均值 5/9（centre_L 差 0.406pp、edge 三格全越）、定心取
  值 4/9；实测 S→L 趋势 centre +0.738/edge −0.163/corner −0.227pp
  无候选复现；R1 回考 +0.703pp 干净失败、R2 +1.578pp（留出幅警
  示）。**勘误**：轮 4 引擎 raw-distortion/fill 转录陈旧（末 knot
  0.9478149 非 0.9420780、fill 0.9774781 非 0.9758350，首方重提取
  lensmeta/lcp/render 三处过等价测试）——轮 4 engine 系 0.1pp 级算
  术与「engine-inverse 1.17px 最优」排名作废，camera/LCP 向量与中
  心律结论不受影响。**零修复授权**（尺寸律未定形，动锚定必双计；
  现役臂 MaskFrame::downstream render.rs:2615-2642+MaskUnwarp
  :2650-2700 将来须整臂反推，涉 Radial/Linear 两类）。105mm/质心存
  活性零码变自保、但对新方向未重测=待实现批。**下一判别测量**：低
  纹理/平坦场景重复 edge_S（同孪生同扫掠），分离探测噪声 vs 真各向
  异性——**用户已拍板（2026-08-23）：供平坦场景 24mm ARW 继续追**；
  尺寸律函数形桌面扫描批（零素材，预注册预测为义务）同时在飞。
- **D2 轮 4 稠密圆锥线判决=六图全灭，但事后分解掘出中心律 12/12
  （2026-08-23，零素材桌面批；报告
  `~/.claude/plans/r30-materials/d2-conic-report.md`，探针四件
  `d2_conic_probe.py/json`+`d2_conic_tables.md`+
  `d2_nested_predictions.md` 同目录归档，仓库根已清）**。批况：Codex
  算完六图×12 椭圆后中转 5/5 重连失败阵亡（244,779 tokens，报告未
  写）；主审全读 265 行脚本+输入逐位对账+本地确定性重跑（G5 0.55/
  original 32.20/R2 89.42 px 与坠机前表逐位同）后亲裁。**三条独立杀
  证灭掉稠密过图全族**：①联合残差——最好 camera_forward 网格中心
  0.55-5.81px 但 original/R1/R2 拟合中心 32/29/89px（标量变体最优增
  益 0.252、残差仍 19.6px RMS）；②非椭圆度——预测轮廓拟合 RMS
  7.7-21.5px vs 实测扫掠 <0.6px+关臂 0.686px；③各向异性——六图预测
  x/y 膨胀差 1.28-3.88pp vs 实测 12/12 全等距 |x−y|≤0.104pp。场平均
  等价 ≤0.2px=连带灭掉「场沿边界平均」整等价类。**事后分解（预注册
  待轮 5 前瞻确认，非定案）**：**中心律 12/12 零参数**——蒙版中心按
  预校正帧点直传过像图，全 12 椭圆 ≤2.77px（engine inverse RMS
  1.17px 最优、camera/lcp forward 1.64px，同向三源系统差内不可分；
  反向 39.8px 死）——含圆锥拟合中心差 32/89px 的三大椭圆（拟合中心
  是错观测量，点心才是对的）；**尺寸律 10/12**——等距化（x/y 均值）
  膨胀中九格+original 全 ≤0.18pp，R1 −0.70/R2 −1.58pp 未解（R2 有出
  幅警示，R1 无=最锋利开口）；同尺寸网格律 h(t) 单调且等 t 格
  ≤0.12pp。**零修复授权**（中心律事后+尺寸律不全；将来修复须重推
  MaskUnwarp 后校正臂防双计）。**轮 5 嵌套实验已预注册**：三中心
  (G5/G4/G1 同位)×三尺寸 S/M/L 九径向单孪生对；判据=中心位移尺寸无
  关（点律）vs 尺寸相关差 ~15px（拟合心律）+定心尺寸趋势分辨尺寸律
  函数形；同心堆叠须选每层曝光量保台阶过闸（侧车批自查）。105mm/
  质心存活性不变（零码变）。
- **D2 轮 3 网格判决=四族预注册全灭+位置律曝光+预校正帧锚定强线索
  （2026-08-23，用户九格孪生导出当日交付；报告
  `~/.claude/plans/r30-materials/d2-grid-verdict-report.md`，18 轮廓
  全 240/240、树净亲验、全 SHA 复核含两张新导出首录）**。**关臂完
  美**：九格中心偏移 RMS 0.686px/max 0.842px=帧锚定第三次实证。
  **位置律（同尺寸 1045×570px）**：帧心 G5 **−2.4%（收缩）**、上下
  边中 −0.5%、左右边中 +1.9%、四角 **+3.1%**；中心一律向帧心移
  （角 ~19/12px、G2/G8 ~29px）——膨胀非恒正、非标量 pad 铁证。**四
  族按预注册表处决**：empirical4/Camera+fill/LCP+fill/aniso 最差臂
  17-29px 超闸全 KILLED。**新有限点过图族=中心近命中但整体出局**：
  轴端点×引擎逆图把 G1/G3/G5/G7/G9 中心打到 0.55-4.5px（=蒙版中心
  按预校正帧变换的强线索），但两轴膨胀差 ~1pp、R1/R2 中心 29/101px
  灾难性失配。**主审裁决轮 4 方向（零素材桌面分析）**：报告自列
  「完整弯曲圆锥线过图未测」——稠密轮廓过图（240 点全过图再量测，
  端点近似的 ~1pp 膨胀差恰是弯曲效应量级）+「场沿蒙版范围平均」等
  价形式，定性上可同时解释 12 椭圆（大蒙版边界扫过强畸变区拉高均
  值：R1 +0.95/R2 +2.08/原 +2.51 均落其边界区间均值附近）；胜=机
  理定案+修复 dispatch 设计，负=嵌套尺寸三中心实验包（报告已给
  规格）。105mm/质心存活性结论不变；零修复授权。
  （2026-08-23，报告 `~/.claude/plans/r30-materials/
  d2-fitfamily-report.md`，分析批树净亲验）**。判据核心：三椭圆所需
  帧心标度 g(t) **非单调**——R1（t=0.415）需 0.994<1 而近同半径的
  R2（t=0.387）需 1.005>1、原椭圆（t=0.630）需 1.007——任何低阶单调
  径向族**数学上不可能**同时命中三者，与中心约定（帧心/存储心/
  DefaultCropOrigin±/自由心）、各向异性 x/y 双多项式、裁切填充复合
  （fill s≈0.9924-0.9929）、帧转换次序全部无关；五族联合拟合残差
  20-47px（探测器系统差 2-3px）契合此判据。既往被拒图的帧序审计=
  无一是次序 artifact。**主审补充假设（预注册供网格检验）**：膨胀
  量与蒙版尺寸正相关（归一半径 0.16→+0.95%、0.30→+2.08%、
  0.34→+2.51%）⇒疑似 LR 对径向做**角点/包围盒过图**（大蒙版跨进
  更强畸变区）而非标量场——与 R26「bbox=角点」同构。**下一步=九径
  向网格实验**：同尺寸 (0.055,0.045) 九格（钉死尺寸变量）+1 字节
  关臂孪生；四族逐格位移**预测表已预注册**在报告——任何不匹配位移
  直接灭对应族、关臂应全钉存储；侧车包构建批在飞。105mm 存活性/
  质心测试结论不变；零修复授权。
  （2026-08-23）**。①用户拍板轮 2：先零素材桌面分析——判别批只灭了
  两个**具体**径向图+仿射兜底，尚未试**拟合**径向族；720 对应点拟
  经验单调径向图（帧心/替代中心/自由中心多约定）+裁切填充约定变体
  +帧转换约定审计；某族在系统差内成立⇒机理族定案+对下轮用户实验做
  **预测性**设计；全灭⇒改派单侧车 5-9 小径向网格高密度采样实验
  （一轮导出测全场）。分析批在飞。②CodeEraser 1.0.1（guard deny，
  2026-08-17 档位切换生效）拦下 6936 行 ROADMAP 追加=用户拍板拆分：
  主文件只留近期五条+常驻参考（441 行），历史「当前状态」条目/已完
  结轮计划/已完成功能①-⑤/2026-07-06 差距调查**逐字**迁入
  docs/ROADMAP-archive.md（拆分脚本多重集断言=每行恰落一侧、
  check_docs 拆前拆后双 20P0F）；archive 入 `.ceignore`（人已裁决
  豁免硬预算）。
- **D2 判别实验收官=决定性否定+机理仍未识别（2026-08-23，用户孪生
  导出当日交付；报告
  `~/.claude/plans/r30-materials/d2-discriminator-report.md`；分析批
  树净亲验、六原件+四侧车+两导出全 SHA 在案）**。素材=侧车批 c/d 对
  （两新径向 R1 小偏心/R2 大椭圆、XMP 恰 1 字节差 @4906、引擎亲读
  回读中标）+用户 LR 双导出 8448×6336。**测量**：三个几何激活椭圆
  膨胀各不相同——原 +2.5083/+2.5235%、R1 +0.9526/+0.9705%、R2
  +2.0796/+2.0718%（R2 191/240 可用角、裁切截断已如实披露）；关校
  正臂两新径向拟合几何钉存储（中心 ≤0.7px、尺寸 ≤0.15%；R2 原始
  RMS 受暗景边缘污染 2.37px 内点法澄清=探测器系统差非几何失效）⇒
  **帧锚定前提保住**。**五候选全灭**（三椭圆中心移动+局部尺度联合
  检验）：帧心等比 s=1.002423 残差 23-44px；蒙版自心 size-pad（各
  1.02512/1.00956/1.02078）过尺寸但中心败 8.45/25.94/28.71px；
  CameraMetadata 图 16.6-59px；已装 .lcp 图 15.5-61px；全局仿射兜
  底仍 22.9-33.5px——**裁决=none，LR 几何激活径向机制在测试族之外**
  （位置依赖 pad/未建模裁切填充约定/另一畸变族三选未定）。零修复授
  权；若来日定案，最小 dispatch 边界=geometry_active && Radial（无
  证据及于 Linear/Brush/位图；105mm 0.99956 相似度否决通用 pad 不
  变）。**⚠恢复首项=用户决策**：D2 闭合需再多轮实验（更大天空 R2
  导出/105mm 重跑/第二台元数据帧）且收敛无保证——「继续追实验」vs
  「重议 v1.0.0 等 D2 的门槛（文档化已知限制发版）」待拍板。本条
  为 2026-08-23 关机暂停点。
- **🔒 v1.0.0 改为等 D2 闭合再发（用户拍板 2026-08-22）+ 会话暂停点
  （记录进度/保存结果/等用户指示）**。**D2 侦查收官**（报告
  `~/.claude/plans/r30-materials/d2-recon-report.md`，RECON ONLY、树
  净亲验）：缺陷坐实——LR 几何激活径向 +2.5083%/+2.5235%（240/240 点、
  24 组探测器扫描 2.4975..2.5462% 稳定），引擎钉存储 0.10/0.36/1px=
  D1 armA 控制独证；**机理=诚实短名单未决**：CameraMetadata 前向预测
  +2.835/+2.353%、已装 24mm `.lcp`（ScaleFactor 1.027391，
  PreferMetadataDistort=True）预测 +3.040/+2.583%，**都不中**；LR 实
  测需 ~等比 1.02516×+(−6.2,−5.7)px 平移，两仓内模型都给不出；D1 式
  度量偏斜被排除（方度量重算反离 LR 更远；径向评估器本就轴归一无点
  积投影）；**通用 +2.5% pad 被 105mm 验证否决**（0.99956 相似度 vs
  像素场 +87.5px——中心质心单测保绿也救不了）；CameraMetadata-only
  规则=仅一帧支撑，须第二例元数据源径向实证后才许实现。**判别实验=
  同 24mm 帧两个新径向（异心异径）+开关孪生**——侧车构建批在飞（c/d
  对、单属性孪生、家规=先 CLI 回读验证再交付+操作步骤）。**同批落
  档三项拍板/勘误**：①猫图 AI 裁切保留当构图卖点写（W3 规格）；
  ②中转令 scoping 勘误——「AS 内部角色走中转」仅限本 session（图像
  流端点 3/3 零 partial 坏死→图生成经用户拍板回直连官方 API；会后
  AS 回 OAuth+用户自设官方 API），前条「不许静默回退直连」指批内自
  作主张、不覆盖用户拍板；③展示 2b 收官=第二张风格三联成
  （DSC09938，judge Revise 成品如实标注、无存配方）+付费帽分毫不差
  （A 6 次/B 3 次）。**Part B 直连终批（2c）收官（暂停协议内验收）**：
  两张端到端全成——官方 gpt-image-2 出图 3520×2352（各首发即中，
  `input_fidelity` 不支持由 CLI 自动降参重试）→ match（CLI 自吞尺寸
  差零手工缩放）→ apply 全 RAW 9504×6336；fit=DSC09938 look error
  0.060→0.042 置信 0.747、_DSC0070 0.172→0.021 置信 0.870；四三联+
  联络表齐（合成 667KB）；**主审目验记录：_DSC0070 反推面板太阳品红
  晕伪影（W3 采用与否待用户，日落张更宜作 Part B 头图）**；付费线系
  =中转 3 败+官方 2 成、零越帽。暂停时在飞仅剩 D2 侧车包批；恢复后
  队列=D2 用户实验→定案→修复批→⑦ 线性重试→W3 README（两类展示
  素材已全齐）→W4→v1.0.0+安装包→官网。
- **D1 线性像素度量根修已落地（2026-08-22，feat `ecb6505`：斜向半
  轮廓误差 874px→9.8px；渲染硬变更=主模型全读 diff，追加二例外条款
  首例执行）**。阶段一确诊主审假设：旧引擎沿自家归一化法线斜率本就
  正确（闭式预测 1964.58px vs 旧引擎控制实测 1959.42px、差 5.2px），
  874px 级差全为归一化↔像素度量投影、非衰减族差异。修=
  `mask_weight_metric`（render.rs:3381）成参数化蒙版唯一生产评估器：
  Linear 点积改像素/长宽比度量（对纯缩放不变=同长宽比预览/导出零
  漂）；轴对齐/方幅/非法尺寸/退化端点逐字保留旧算术（主审 sed 亲比
  =字节稳定）；unwarp 门控 match 臂原样保留=帧判定零变更；径向/笔刷
  /位图/AI/组合语义零触碰、零 schema；dims 由 apply_masks/
  mask_coverage 两入口穿线（签名变更=全调用点编译期核走，GUI
  coverage overlay 与渲染权重同路径）。验收=armB 控制法 1095.08px
  vs LR 拟合 1085.25px=+9.83px 残差（armA 绝对配准 −120px 搜索边界
  饱和=无效标尺、如实拒用；armB 径向存储偏差 0.5083/0.7583/2 字节
  稳定=径向未动实证）。⚠**渲染硬变更：非方帧斜向线性蒙版配方
  v1.0.0 全变**（README 硬变更清单已载）。主审全读抓错一处：批写的
  README 段把 D1 错归因「.lcp 模型几何关断时用于线性渲染」——D1 纯
  度量修复不消费 .lcp，已改回 carried-provenance+度量事实独立成句。
  门主审亲跑=clippy×2+852(9i)/14/132/2+2 双配置（849→852 集差=+3
  具名：angled 闭式/轴对齐字节稳定/GUI overlay 一致，−0，diff 亲证）
  +python 4P+check_docs 20P0F3S+2 变异亲手（M1 py 错用 w=闭式+
  overlay 双红、M2 回退臂 len2×2=轴对齐红；还原后 3 测试绿+diff 恰
  回 248 行）。报告归档 `~/.claude/plans/r30-materials/d1-report.md`。
  D2（几何激活径向 2.5% 膨胀、CameraMetadata vs .lcp 源分叉假设）仍
  列 v1.0.0 前置队列；⑦ 线性场锚定接线重试排 D1/D2 后（LR 端证据
  不变）。
- **W2 展示批 1 验收通过+新规格 v2+三用户令落档（2026-08-22）**。
  **批 1 验收**（Codex 全程 dist exe 0.35.0+隔离 state，repo 零写
  亲验）：8 张批准照（用户逐张拍板「就这几张吧」：DSC09938/09659/
  09709/00001、_DSC0070/0311/0598/0639——仅此八张可入公开 README）
  neutral+AI 全尺寸+8 对 1600px pair+_DSC0070 反推三联（look error
  0.172→0.020、置信 0.87，批自明=look recovery 非参数复原）+联络表，
  合成资产 1.947MiB；主审亲验=尺寸×3/配方字段×2/蒙版名×2 抽查全中
  +联络表/三联图亲眼目验（报告
  `~/.claude/plans/r30-materials/showcase-report.md`）。披露四项=
  advisor 未提任何 AI 位图蒙版（该卖点需另证）；AI 自主裁切（猫图
  [0.14,0.14,0.72,0.91]=构图决定，去留待用户）；首张 Claude OAuth
  verifier 断连废一次付费 proposer（余走 gpt-5.6-sol verifier 回退）；
  **D1 已落地⇒批 1 的 8 张 AI JPEG 含斜向线性者须按存档配方重渲后
  方可入 README**。**展示新规格 v2（用户令）**：README 展示图定为两
  类各 1-2 张——A=「AI 分析」+风格读取（style-index 建自
  D:/Photography/Raw 147 RAW+XMP 对（M-C 同源）、2 张×style 0/0.5
  对照三面板）、B=「AI 整图生成」reimagine→match 反推三联（同图三
  态：直转/生成低清/反推配方全分辨率重渲）——批 v2 在飞。**中转令
  （用户）**：本阶段图像 AI 一律走中转 API（key/端点只入内存、文档
  一律写「中转」、付费生成前探 /models 不在列即 STOP，不许静默回退
  直连）。**技术栈令（用户）**：README 与官网各开独立技术栈展示
  section、细节算法一起展示——两令均已入 W3/W6 规格与 ccm 12 步计划。
  **追加二分工令（用户）**：普通实现批复审改「独立 Codex 只读对抗
  复审+主模型读裁决/定向抽查」；渲染硬变更与 schema 断裂批仍主模型
  全读（本 D1 批即例外首例）；门/变异/裁决/台账/提交纪律不下放。
- **R30⑦ 镜头开关孪生实验收官=径向免接线+线性接线 STOP+两个一阶
  新缺陷（2026-08-22，分析批+接线批各一，零代码落地；报告
  `~/.claude/plans/r30-materials/mw-{analysis,wiring}-report.md`+素材
  `mw-exp/` 六件 SHA-256 在案）**。素材=用户 FE 24-105 @24mm 单开关
  孪生对（sidecar 唯一字节差分 @4906 `LensProfileEnable` 1→0，exp_D
  同工艺；首选 Sigma 14-24 因内置强制校正做不出关臂而换镜，其 A 臂
  转作 ④ 用片）。**像素分析**（195 NCC 场点、胡子形场实测、三检测器
  带系统差自检、诊断图主审目验）：**径向=帧锚定**——LR 关校正臂钉在
  存储椭圆 1.78px RMS ⇒ 引擎现行无几何按存储坐标渲染即 LR-faithful，
  **免接线**（⑦ 径向支闭）；**线性=场锚定中等置信**（场假设 3.27px
  vs 钉帧 13.89px、场重采样残差 1.20px）⇒ R29「径向/线性同帧」捆绑
  被拆（当年像素证据只测径向、线性系类推）；**笔刷对照**场 1.01px
  完美命中=方法自验。**接线批按 STOP 条款撤回**：候选（MaskFrame 增
  几何未激活+mask_warp 非空态、仅 Linear 走 `lr_mask_unwarp_norm`、
  机制单点覆盖四渲染面）机械正确（中点映射 +30.5px 亲证）但 armB
  引擎验收咬不动——**掘出一阶新缺陷 D1：引擎线性渐变在归一化坐标算
  点积、LR 用像素几何**，非方帧带角渐变整体拉斜（引擎半衰减轮廓
  1959px vs LR 1085px、带宽 2203px），⑦ 的 14px 锚定修正是其二阶量
  ⇒ 线性接线**阻塞于 D1 根修后**（LR 端证据不变仍有效）。**armA
  探针掘出一阶新缺陷 D2：几何激活径向 2.5% 膨胀不复现**——LR armA
  把径向椭圆膨胀 +2.51/+2.52%（30px 量级），引擎钉存储椭圆
  0.36px RMS（本例 `mask_warp_src=CameraMetadata`；R29 B3 的 0.09-
  0.30px 验证是 105mm `.lcp` 源，两源行为可能分叉，机理未定）。
  **D1/D2 均列 v1.0.0 前置渲染修复队列**。批门=clippy×2+
  849(9i)/14/132/2+2 双配置+python 4P+集差 added=[] removed=[] 亲报；
  `--gates` 暴露 README:542/ARCHITECTURE:82 电池数 816 陈旧（B1-B3
  漂移）→W4 全量彻查项。树净（撤回后 diff 空亲验、纯换行噪声
  checkout 归位）。⑦ 素材另产出=R26 bbox 解码再获 240 角点级验证。

> 📦 更早的「当前状态」条目、已完结的轮计划（第十/十二/十三/二十二至二十四轮）、
> 已完成功能 ①-⑤ 与 2026-07-06 Photoshop 差距调查已整体迁入
> [docs/ROADMAP-archive.md](ROADMAP-archive.md)（追加式历史台账，逐字保存，勿重写）。
> 拆分依据：用户拍板 2026-08-23（CodeEraser 750 行硬预算 × 台账追加实践之争）。
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
