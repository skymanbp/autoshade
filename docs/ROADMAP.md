# ROADMAP — “一定程度直接取代 Photoshop” 路线（v0.5.0 之后 · UX 阶段）

> 交接文档：每项都附实现要点与 `file:line` 锚点，供新会话不重读全库即可
> 开工。更新于 2026-08-24（**v1.0.0 已发布 2026-08-24**，tag → `9128cff`，
> 四资产回下载字节验证——硬变更/schema 断裂清单见本文件尾「v1.0.0 发版义务
> 清单」与 docs/RELEASE_NOTES_v1.0.0.md；官网 autoshop-d7w.pages.dev；
> 此前 **v0.35.0 已发布 2026-08-21**，tag → `e75f728`，
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
| **M-B** | 一次 LR 采样实验的 sidecar 族：①径向旋到已知角度 ②径向组件组合 ③含局部点曲线的蒙版 ④AI/画笔蒙版 | ✅ **全部交齐**。R24 自采 160 份取证覆盖③④与阴性结果；**①已知角度 ✅ 2026-08-18 十二张受控导出已交，已在 R26 v0.32.0 全部消费**；~~Defringe 非零仍欠~~ **← 2026-08-19 勘误：Defringe 非零同在 2026-08-18 那批里已交**（`P17.xmp: crs:DefringePurpleAmount="10"`、`DefringePurpleHueLo="19"`、`DefringePurpleHueHi="49"`；同批另七份全带静息块 `0/30/70` + `0/40/60`，反过来独立确证 R25 选的非零默认）。该行原是一次部分编辑的残留，不是判断 |
| **M-C** | `f944ef3` 那 147 张 RAW+xmp 对照集是否还在、路径确认 | ✅ R25 全量重跑（数字见 v0.31.0 条）→ **R27 第三次尝试落地成新基线（2026-08-19）**：HEAD `f43dd85`、中转端点（gpt-5.6-sol）+ `--jobs 3` 并行、147/147 全 done、**双流零回退**、约 38 分钟；`~/.claude/plans/r27-materials/mc-eval-147-r27.txt` 自此为唯一参照，R25 降为历史。前两次尝试在案：#1 死于 83/147 commit 内存墙（Batch-7 已根治）、#2 有 2 条中转上游 `stream_read_error` 回退按判据废稿。头条：**gap 15.5%；蒙版拒收 2.35→0.05 张/照（Batch-4 导入根修实证）；whites 偏置 +22.55→+16.20、blacks −14.18→−10.87** → **v0.34.0 重基线接替（2026-08-20，B1a 重试+续传首个实战）**：attempt-1/2 各废于一条中转流错（`response.failed`，判据执行）；attempt-3 单进程 147 全 done 但 **5 回退**（4×中转 http 524 计费不明类＋1 畸形 JSON）+ **1 硬败**（Claude 529 过载）＝报告混入启发式行弃用；**续跑 141 行免费载入＋6 张补买零回退**（提源行 `147 rows: 141 loaded, 6 measured, 0 fallback`）→ `~/.claude/plans/r28-materials/mc-eval-147-v0340-resume1.txt` 为现行唯一基线（同目录 attempt-3 全量转录留档），R27 降为历史。头条：**gap 16.0%**（vs R27 15.5%——R28 5b 四条 `color_grade.*_hue` 行口径改两侧过阈自报跨版不可比，其余行可比）；whites +16.12 / blacks −10.60（与 R27 同量级）；蒙版拒收 0.05 张/照持平；AI 蒙版 1.99 vs 用户 2.30 张/照。**登记 R29 候选**：524/529 类非流死亡瞬态传输失败是否纳入重试（本次漏斗 6/147≈4%，524 属计费不明须拍板；已落 `74ffa25`）；W_EMB 两臂标定仍欠第二臂（已于 v0.35.0 判分窗闭合，见下） → **v0.35.0 重基线接替（2026-08-21，拍板八，发布位 exe）**：`~/.claude/plans/r29-materials/mc-eval-147-v0350-resume1.txt` 为现行唯一基线（`147 rows: 145 loaded, 2 measured, 0 fallback`），v0.34.0 降为历史。头条：**gap 17.7%**（vs 16.0%——**⚠ 差值=verifier 效应+采样噪声，与 R29 渲染硬变更无关**（2026-08-22 勘误：eval 不渲染像素，proposer 提示词/量表跨版逐字同=`openai.rs`/`catalogue.rs` 零 diff 亲证；verifier 从 oauth/opus 换成中转 gpt-5.6-sol 系 opus 经继承中转 env 产 16% 畸形 JSON 被迫切换，verify→revise 环直接改提案数值），不构成渲染回归证据）；whites +14.85 / blacks −10.91；蒙版拒收 0.05 张/照持平；AI 蒙版 1.99 vs 2.30 持平。W_EMB 标定同窗闭合（离线 leave-one-out，settings 目标平坦 CI 跨零 → **默认 2.0 不动**；细账见「当前状态」判分条 + V2_PLAN §7-8）。判读细读归用户 → **同版本重复跑伴随数（2026-08-22，拍板②付费，非接替）：gap 16.3%**（同 exe/库/模型全新 state 两续传 0 fallback，`mc-eval-147-v0350-repeat1-resume2.txt`）＝同版本重采样摆动 1.4pt 与跨版 +1.7pt 同量级，「非回归」获直接实验证据；基线仍为 17.7% 那份不变，公开引用须双数并报（细账见「当前状态」重复跑条） |
| **M-D** | #2 蒙版精修报错的 toast 原文（分流哪条臂） | ✅ R24 两臂都修 + 实跑否证 |

## 当前状态（已完成，勿重做）

- **🎯 反推估计器换型：对应像素稳健回归（2026-08-26，主模型亲写）**：区内估计器从
  边缘分布匹配（矩/CDF 分位数传输）换为**配对稳健回归**——同帧对上按栅格索引配对，
  逐 64-bin Tukey-IRLS 均值（加权中位数起步、证据×稳健双权、色度族群独立尺度），
  候选 (ev, 滑块) 经**引擎自己的样条**在映射点上按质量加权做模型选择（近共线平局由
  结点间形状拆开）；结点与残差曲线电平在**测得亮度域之外零权**（支持=证词计数≥32
  或配对点覆盖域，非帧占比）；打分环改一致加权最小二乘（旧式 `(r−w·fit)²` 让零权
  结点的原始残差牵引 ev 扫描——p36 畸形解的最后一层根因）。**色相轴新增担保收敛
  教义**（亮度秩配对的同构精化）：逐像素担保=稳健权重≥0.5 + 色相相干（与全局编辑
  主方向偏差≤60°）+ 向自身配对目标靠近，担保像素可穿过单侧零证据带（撤销色偏不再
  被色偏自己发明的带否决），未担保/不相干（真实内容分歧）维持全部否决；旋转门与
  外来色相门为工具能力政策、不受豁免；担保通行与稳健拒绝均为具名披露注（EN+zh）。
  途中定罪并修复 **899798e 引入的 p36 全局解退化**（秩配对首次把该对送进配对通道，
  引爆外推+幻影结点潜伏缺陷：ev 顶格 +3.0/黑 +93.6/置信 0.725→0.25）。**实测**：
  p36（LR 真值对）ev +0.75/置信 0.692/全幅 dE 17.24→**11.49**；四参真值恢复误差
  35.65→**28.85**（黑不再反号、白不再凭空 +44、零补偿曲线、ev 精确 0.50）；干草对
  0.0547→**0.0148**（旧 0.019，饱和+三通道曲线首次全存活）；viaduct 实拍对从终止
  重置变真拟合 0.042→0.026（joint 0.119→0.033）；校准对持平（land dE 7.978 vs
  7.979、天空能量比 0.841≤1.0、边界门 rim 0.013→0.012 预算内保 2 区）；p37 蒙版
  重编辑档诚实入氛围模式（分区/范围蒙版步的领地）、p38 0.029→0.007、p39
  0.054→0.031。门：clippy 0+0、库 **921(912+9i)**（集合差 +4/−0 具名）、CLI 14、
  GUI 144（字体重子集 833/833，新增「担敛权穿维获集」七字形）、契约 2+2、i18n
  九项零、docs 23P/0F；**五变异亲手全红**（MA 稳健→最小二乘、MB 拆披露、MC 组合权
  丢支持、MD 恒相干、ME 拆豁免），sha 复原。三处过时钉如实重钉（canyon 干草复刻段、
  joint 干草档、viaduct 终止段——每处保留保护意图，新契约+实测新值）。**待用户复核
  的教义精化：担保收敛豁免**（若否决，回退面=去掉 vouched 分支，干草对退回 0.020）。
  语料新增 p36-preview.jpg（相机内嵌预览=CLI match 同款基底）。

- **🧰 三项用户报障清账：假提示修真（`e72b387`）、线性黑线结案（零改动）、GUI
  内存与线程预算（2026-08-25，主模型亲写）**：①「已保存的显影存在但不含有效
  编辑」曾只凭活动卡（recipe.json/XMP）就对整个显影下断言——variants.json 的
  后台卡与 pixels.json 的烘焙母版都装着真编辑时照样误报。只改读取侧
  （`persist.rs::noop_only_note`）：烘焙母版在=不出注记（画布本身就是作品）；
  后台卡有活=改说「已保存的编辑在 {n} 个后台变体中」；条带不可读=不下断言
  （未知≠没有，Opened 已另有 toast）；真空=旧句照旧。recipe.json 一字节不改并
  有断言钉住。具名守卫四结局全钉，撤守卫变异红。②线性渐变满端黑线：平灰源探
  针实测渲染无线（满端平台 sd 0.0000000、两角二阶差 0、全帧最大跳变=中段 1/255
  量化步），用户在 Lightroom 同图复现相似现象后裁定为图片内容问题，代码零改动
  结案。③「AS 让整机不可用」根因=GUI 各重活类内有门（busy/big_decode_gate）
  但进程级总和无界，且引擎跑在逐逻辑核的默认 rayon 池上；本机基线 31.2 GB 装
  26.7 GB、余 4.4 GB 提交额度，一趟全幅管线实测峰值 ~1.77 GB。新增
  `src/bin/gui/budget.rs` 仿 jobs.rs：全幅峰值类 12 个 worker（打开解码/分析/
  反推/单张与批量导出/蒙版精修/母版加载/填充/修复/去噪/克隆/重构想）动手前先取
  字节预约许可——首个必放行（下限 1，绝不拒绝）、其后仅当当前空闲内存 ≥ 估计
  +2048 MB 保留时放行，否则在 worker 线程排队（限时等待逐秒重采样；输出字节
  与不排队完全一致，绝不静默降档）；估算=烘焙头部报价与语料常量 1800 取大
  （jobs::survey_peak_mb 同款下限）；启动时若空闲 < 保留+两峰（紧张机）把全局
  rayon 池钳到 8 线程（实测 16→8 单趟 8.03→9.78 s、峰值 −2 MB——买的不是内存
  是换页速率减半）。门主审亲跑：GUI **144**（140→144，+4 具名预算测试）、
  clippy 0、审计与字体门不变；三项亲手变异（准入全放行/线程永不钳/许可泄漏）
  各自变红。**披露**：12 个接线点本身无变异守卫（测试无法廉价观测 worker 内
  部），靠编译期类型与逐点核对表把关。

- **📏 证据门控批系列 + 分类拒绝披露 + 栅格所有权上类型（2026-08-25，用户拍板
  「一批收尾再提交」「实在不行就不用 codex，你直接自己上」）**：反推的证据模型
  从「有无」升级为「逐范围可测性」——17 个亮度桶按**秩**配对目标（等像素数下源
  桶永不目标为空）、8 个色相带独立分类、3×3 结构分歧网格给出逐像素空间支撑；
  目标函数改证据加权（真值回收四参数绝对误差 115.1→21.6，contrast −32.2 →
  +20.9），高架对 0.041910→0.012992。**色带证据循环的根治=按控件类拆否决**：
  亮度探针与色度探针独立渲染判定，单侧色相带只扣色彩类（增益/饱和），有据的亮
  度证据照走影调——生成云层案天空区从整区被丢变为保住 EV 修正。**FAR 按成因分
  型**：`classify_joint_far` 把「主动拒绝导致的远」与「拟合失手的远」分成两句
  互斥披露（E-15：拒绝≠失手）。**披露修真**：一句 `ZONE_EVIDENCE_WITHHELD` 曾
  同时覆盖「色彩扣下/影调扣下/整区不附加」三种结局，且被占比失配出口借去渲染成
  「零证据范围 [none]」——拆成 `_COLOUR`/`_TONE`/`ZONE_SHARE_NO_CORRECTION` 三
  键各说真话，补上从来无人守的影调分支具名测试（夹具按秩配对机理设计：结构分
  歧天空 6144/9216 像素失支撑、全幅无彩、只出影调注记）。**数据丢失事故根治**：
  2026-08-25 一次监督变异令 `accepted.is_empty()` 恒真，测试把用户语料
  `sky-mask.png` 当 owned 路径传入，五处清理点之一将其删除（当日已按原命令重建
  蒙版并经散度读数 1.205776/0.423271 复验）。上游缺陷=删除契约只在文档注释里：
  新增 `store::OwnedRaster`（只能由 `claim` 或测试 `scratch` 构造；`scratch`
  与 `remove` 各拒一次语料目录），反推五入口改收该类型，**生路径直达删除点
  现在编译不过（E0308）**。末批两处测试删除经主审变异裁决均仍有人守（M1/M2
  变红），不构成覆盖损失。语料完整性守卫 `calibration_corpus()` 缺文件时可见
  跳过。门主审亲跑：**917(908+9i)/14/139/2+2**、clippy 0（含 gui）、audit_i18n
  全 0、check_docs 23P0F3S、fonts 826/826；测试名集合差 vs HEAD **+22/−1**
  （−1=裁决过的改名）；四项亲手变异 MA/MB/MC/MD 全达标——**MA 重放事故变异：
  测试红且语料 sha256 前后逐字节一致**。文档同步 README/ARCHITECTURE 电池数
  896→917 + 所有权契约一句。

- **🔲 分区边界连续性门落地 + 「蒙版硬化」机制实测证伪（2026-08-24，用户拍板
  「提交收缩门，撤硬化，另立精修批」）**：上一条登记的可见亮边补批已完成并
  入库——`boundary_rim` 只在蒙版 5%–95% 过渡带内读有符号亮度弓形（对照同行/
  同列的已定天空与地面），超预算 `ZONE_BOUNDARY_RIM_MAX=0.012` 时按**单一标量
  收缩两区差值**（12 步二分、保各区方向），`k=0` 仍失败才具名丢弃；七条具名测
  试，四项主审变异全红。实测该对选中 `k=0.093`。**随后派出的「硬化为主、收缩
  兜底」补批被主审证伪并撤回**：任务书前提（主审此前测得硬化 k=3 鳍幅 p90
  +0.0097、优于无分区地板）**不可复现**——遍历全部（渲染 × 参考蒙版）组合无一
  落在该值；错因是**鳍幅统计依赖参考蒙版**，而被比较的各渲染用的是不同蒙版，
  每张都在自己的蒙版下得分最好。改用**与蒙版无关**的过盈统计（各渲染对自己的
  无分区孪生取差、按自身两侧平台区间外的偏出量；对照读数恰为 0.0000）后结论
  反号：HEAD p90 0.0065 → 硬化 k=3/6/12 = **0.0086/0.0087/0.0089 单调更糟**，
  1:1 目检亦从柔光晕变成**硬白描边**。**根因（比羽化宽度更上游）**：分割蒙版
  的非天空区**膨胀入天空**——其 0.5 等值线相对图像自身梯度边缘内移均值 3px、
  p90 21px，该带永远收不到天空的 −0.64 EV，于是保持明亮＝光晕；硬化把该带从
  「部分变暗」直接吸附成「完全不变暗」，只会让配准误差更锐利。反证：把轮廓向
  地面侧生长 12px 后**全强度**渲染光晕消失（过盈 p90 0.0050，优于收缩门的
  0.0055 而局部强度 100% 而非 9.3%），但均匀形态学会吃掉远景细结构 → 正解是
  **边缘感知的蒙版精修**，另立批次（范围＝只修反推分区用的蒙版）。字体子集因
  新中文披露串补 且/享/渡 三码位重建（--check 804/804→**807/807**）。门主审亲
  跑：clippy 0；**896(887+9i)/14/139/2+2**，测试名集合差＝0 删除 / +7 新增；
  audit_i18n 全 0；check_docs 23P0F；文档同步 ARCHITECTURE §4.8 + 四处电池数
  889→896。

- **🌫️ 反推结构分歧门 + 逐区氛围模式 + 局部质量门落地（2026-08-24，
  `5aaeea4`，用户拍板「按推荐走，但氛围模式也要保持局部区域的质量」）**：
  根因=拟合器所有门只看分布不看结构（同帧判据只是 2% 纵横比、terminal harm
  与置信都是观感误差阶梯、cast 门只扣 RGB 曲线），于是平淡天空与生成云层被当
  作同一总体做 CDF 匹配——上源带 5.79 码值对应目标 86.23 码值（逆 CDF 斜率
  14.9）落成残差段 (149,98)→(170,193) 斜率 4.52，全局 −1.55 EV/高光 +71.7/
  黑色 +16.7 把天空梯度能量放大到中性渲染的 **3.0×**（噪点）。**一统计定策**：
  `structure_divergence` 在蒙版内秩均衡亮度、腐蚀边界、对模糊梯度图做 ±6px
  平移相关，加五带高斯能量误差，D=√((1−corr)²+energy_err²)；标定=展示对
  .075/.168/.095、高架 .070、日落 .226、同内容顶部 35% 条带 .455/.532，失败对
  全局 .491/天空 1.186/地面 .436 → 阈值全局 0.35、分区 0.65、分歧区 ≥35% 帧面
  提升全局。**全局氛围模式**：稳健分位 EV ±1、WB 仅当各增益 ∈[0.80,1.25] 且
  max/min ≤1.40、饱和 ±30、五点曲线斜率 [0.5,1.5]、永不出通道曲线、置信封顶
  0.50，决策与 D 全量披露。**逐区（用户令）**：每区独立判模式，**分歧永不导致
  丢区**；分歧区=氛围区（EV ±0.75、增益按**单一标量**向 1 收缩保方向
  [0.85,1.18]、不做区内 CDF、饱和 ±20、只要求不恶化）；**所有区所有模式过局部
  质量门**（蒙版加权纹理能量比 ∈[0.70,1.95]、裁切份额增幅 ≤1pp，各自具名披
  露）；与模式无关地把每条残差曲线投影到 2:1 斜率上限（不删点、保端点与单
  调）。实测：全局 D=0.492 氛围、天空 D=1.207 氛围（EV −0.64、增益
  [1.18,0.92,0.85]、饱和 −5，区残差 0.125→0.008）、地面 D=0.423 完整
  （0.025→0.000），两区质量门 0.920/1.051 无裁切增长；天空梯度能量 3.0×→
  **0.83×**（噪点在源头消失）；三张同内容展示对配方逐字段不变。**主审在批之上
  的两处修正**：①批把五处机器绝对路径写进测试（含主目录、显影库 ID 与 RAW 文
  件名，**公开仓库**）→ 改为单一环境变量定位的校准语料
  `AUTOSHOP_FIT_CALIBRATION_DIR`（规范文件名 neutral/target/fitted.recipe/
  sky-mask/source.arw，沿用 check_docs 的 `AUTOSHOP_CENSUS_ROOT` 惯例），源码
  绝对路径字面量归零；②批自身测试**漏掉两条预算**——把 ATMOSPHERE_SAT_LIMIT
  30→60、RESIDUAL_SLOPE_CAP 2.0→10.0 均无测试变红 → 增两条具名测试
  （`atmosphere_saturation_cap_is_load_bearing` 用超预算色度需求要求落在 ±30
  且披露顶格；`residual_tone_curve_projects_a_cliff_through_the_real_producer`
  驱动真实生产函数而非投影助手）。门主审亲跑：clippy 0；**889(880+9i)/14/139/
  2+2**（+16 批测试 +2 主审测试）；audit_i18n 9 项全 0；check_docs 23P0F；
  **七项亲手变异全红**（分区模式强制 Full→3 红、去增益收缩、绕过纹理门、抬饱
  和上限、抬斜率上限、去置信封顶、删投影调用）。文档同步 README 反推段/
  ARCHITECTURE §4.8/TECH_STACK + 四处电池数 871→889，官网重部署 9dad0c27 字节
  全同（sha256 c03e4b21…）。**已知缺陷（补批在飞 `task-fit-boundary.md`）**：
  两区反向移动时，分割蒙版的软边过渡带让天空侧吃进地面的提亮，沿高反差轮廓出
  现可见亮边——主审量化（1600×1067、用户存档天空栅格、180 处过渡）：旧反推
  mean +0.0044/p90 +0.0082，本次 **mean +0.0457/p90 +0.1256**，无分区 +0.0016；
  换成批自己生成的蒙版数值不变（0.190±0.066 两者相同）=非蒙版文件问题而是**区
  对差值跨软边**；区内质量门按构造看不见薄过渡带（该天空以 0.920 通过）。补批
  =边界连续性门（过渡带鳍幅测量 + 按单一标量收缩两区差值，保方向、可披露、
  不可收缩才丢弃）。
- **🔁 变体 ● 误报修复 + 变体/版本语义澄清落地（2026-08-24，用户拍板
  「派 Codex 修」「并入 ● 修复批一起改」）**：根因=四处未保存判定（状态栏 ●、
  关窗守卫、导航暂存门、退出对话 PendingSave）都拿画布比 `saved_recipe`/
  `pixels_on_disk` 单槽，而该槽只描述保存时的活动卡片；Ctrl+S 早已把其余卡片
  存进 `variants.json.others`，所以切卡即被当成编辑。一次系统性改动（Codex
  gpt-5.6-sol xhigh 写批，主审全读 diff）：`actions.rs` 新增唯一 owner
  `active_baseline()`（id 优先；无 id 旧记录回退 kind+位置，与 `version_is_from`
  同序；无记录时仅平凡孤卡映射 recipe.json/pixels.json；保存后新推的卡片无基
  线=未保存）＋ `active_canvas_dirty` 承担配方与像素两半，四处调用点各走具名
  谓词（`unsaved_marker_dirty`/`quit_guard_open_dirty`/`nav_stash_gate_dirty`/
  `pending_save_gate_dirty`），无一保留自己的单槽比对；`open_dirty_variants`
  改按身份与「记录活动卡 ∪ others」联合比对，不再比 `active_pos/kind/id`——
  裁定：**切卡=导航不算未保存**，选择只随下次保存落盘，**重开落在最近保存的
  活动卡而非最后浏览的卡**；改名/新推/删卡/配方/像素来源变化仍算未保存
  （R24-3 不变）；`variants.json` 格式零改动、store.rs 未动。UI：Versions &
  Export 分「变体（卡片）」/「快照历史」两栏（批写的 zh「条目」由主审改回「卡
  片」；「卡」U+5361 原不在嵌入 CJK 子集，`embedded_fonts_cover_every_ui_symbol`
  红 → 按 `scripts/subset_gui_fonts.py` 文档自 google/fonts ofl 树取五个捐赠
  字体重建 `assets/fonts/` 五个子集，`--check` 804/804、SC 子集 737 码点，四个
  符号子集字节数不变仅哈希变）、＋ 悬停明示只快照本卡、条带与 ● 悬停明示
  Ctrl+S 保存全部卡片，中英齐全（audit_i18n 0 问题）。README §4、ARCHITECTURE、TECH_STACK 同步改写语
  义；四处电池数 132→139 GUI（README/ARCHITECTURE/TECH_STACK/site），官网重部
  署 a5179c1c → 别名 autoshop-d7w.pages.dev 首页与 site/index.html 字节全同
  （sha256 a2819e4e…、29571 B）；assets/fonts/README 计数刷新。门主审亲
  跑：clippy 0；871(862+9i)/14/**139**/2+2（GUI +7 具名：
  switching_persisted_cards_keeps_all_four_unsaved_consumers_clean /
  edit_save_and_switch_back_use_each_cards_persisted_develop /
  pushed_and_deleted_cards_remain_unsaved_strip_work /
  persisted_card_ids_outrank_kind_and_position /
  legacy_idless_strip_uses_kind_and_position_baselines /
  switched_generated_card_uses_its_own_persisted_pixel_origin /
  renaming_a_switched_to_card_is_still_unsaved_work；既有测试零改动）；三项亲
  手变异全红（● 回指单槽→6 红、去 id 优先→1 红、去像素半边→1 红，文件字节还
  原）；check_docs 23P0F；公开文档照片文件名 grep 0。行为变化须知：三处决策点
  不再逐次读盘 `read_pixel_source`，改用逐卡镜像基线（镜像在 open/save/
  analyze-save/clear 推进，写失败不推进故保护仍武装）；原 pixels.json 的
  generated 标志比对随之取消——标志=kind，而 kind 漂移已在 `open_dirty_variants`
  按卡计脏。登记跟进：`src/bin/gui/i18n.rs` 1748 行超 cc-enforcer 750 行硬预算
  （本次 zh 一词改动经脚本落盘、行数零增长），拆分另立批。用户复测待办：切
  卡不再 ●、重开落在最近保存的卡。**反推氛围模式**：Codex 只读诊断批报告因
  `-s read-only` 拒写而内联返回、主审落盘（`~/.claude/plans/r30-materials/
  fitatmos/task-fit-atmos-report.md`）：根因=拟合器把原片平淡天空与目标生成
  云层当同一分布做 CDF 匹配（源天空 IQR 0.0764 过 0.05 守卫；上源分布 5.79
  码值对应目标 86.23 码值，逆 CDF 斜率 14.9 → 残差段 (149,98)→(170,193) 斜
  率 4.52 即断崖），现有门（同帧 2% 纵横比、方差 1e-6、cast 门、terminal
  harm .224、联合误差、置信）全部只看分布不看结构；结构分歧统计 D=√((1−秩均
  衡梯度相关)²+五带金字塔能量误差²)：失败对全局 .491/天空 1.186/地面 .436，
  同内容展示对全局最大 .226（顶部 35% 条带最大 .532）→ 阈值全局 ≥0.35、分
  区 ≥0.65、分歧区 ≥35% 帧面提升全局；氛围预算 EV ±1/WB 增益 [0.80,1.25]/
  饱和 ±30/五点曲线斜率 [0.5,1.5]/无 RGB 曲线/置信封顶 0.50；独立门=残差曲
  线斜率上限 2.0（三张同内容展示对最大 1.905 不受影响）。原型主审 CLI 真渲
  （contact-sheet.jpg 已发用户）。**用户拍板（2026-08-24）：按推荐走，但氛围
  模式也必须逐区保质、不得整图一刀切** → 主审设计=一统计 D、全局/分区各自
  模式枚举、分区永不因分歧被丢弃（分歧区=氛围分区：EV ±0.75、增益向 1 收缩
  保方向 [0.85,1.18]、不做区内 CDF 调子、饱和 ±20、只要求不恶化）、**所有分
  区所有模式过局部质量门**（纹理能量比区间 + 裁切份额增幅上限，常量按夹具
  与失败对标定）、残差斜率上限 2.0 独立生效；任务书 task-fit-atmos-impl.md，
  写批（workspace-write）在飞。
- **🧹 展示图注去文件名 + 官网重部署（2026-08-24，用户令「官网/readme 上不
  要出现我自己的照片文件名」）**：README 13 处 + site/index.html 13 处（三对
  分析对的标题/alt、两张风格三联图注、两张反推三联图注）由脚本逐条断言计数
  替换为场景名（Townhouse and pond / Balcony view / Hillside neighborhood /
  Lake and boat / Sunset / Stone viaduct），两文件 `_?DSC\d{4,5}` 余 0；
  check_docs 23P0F；部署 a38fd7a0 → autoshop-d7w.pages.dev 首页与 site/
  index.html 字节全同，www 仅多一段 Cloudflare Web Analytics beacon（zone 级
  自动注入，被站点 CSP `script-src 'none'` 拦截，非站点内容）。用户随后裁定
  「全部清理」：ARCHITECTURE 4 / ROADMAP 1 / ROADMAP-archive 58 / bug 模板
  合成例 1（→ `photo.ARW`）+ 留盘的 V2 11 / M1 6，35 个原文件名统一映射为
  稳定别名 `P01`–`P35`（后缀保留；脚本白名单+断言计数，越界即拒；别名↔原名
  对照表只存用户目录，不入库），全部公开文档大小写不敏感残留 0。同日
  用户 ④ GUI 目检通过 → v1.0.0 计划 #12 用户侧闭合。同日用户报三问题待拍
  板：变体切换后 ● 误报（根因=● 只比 `saved_recipe`=recipe.json 单槽，
  app.rs:1192，而 Ctrl+S 已把其余卡片存进 variants.json）、变体/版本语义
  混淆、反推质量差（沙漠峡谷片：目标含生成云层=内容差异，统计拟合结构性
  失败，置信 0.458）。
- **✅ v1.0.0 发布后彻底收尾（2026-08-24，用户四项令）**。①**全量文档深度漂
  移审计**：只读对抗批 205 条主张双侧 file:line → 1 BLOCK（「库只读」保证漏
  交付目录设在库内即可写的例外）+20 FIX（README 兼容披露缺重渲范围/精度
  数字、ROADMAP 常驻节陈函数名 apply_lens_distortion→geometry_profile→
  apply_lens_geometry、distort_norm→view_to_original_norm 族、v0.3.0 陈句、
  V2 §7 已证伪径向律未标历史/MaskBrushTable 与手势提示未标 CLOSED/
  is_lr_post_correction_geometry 已降测试专用、M1 §7 陈计划与三处陈锚、
  bug 模板死锚+陈笔刷披露+recipe.json 复现过誉、main.rs GUI 路径、decode/
  serve Clap 文案 RAW-only 不实、xmp.rs「未发布」、denoise.py 输入实为
  sRGB-gamma 桥）+3 NOTE（三前端口径统一；三张旧展示对分数无运行记录→
  图注改「产出时模型评审分」；远端资产由主审 gh 验证关闭）；修订批
  111d588 全部落地，门主审亲跑 check_docs 23P0F/clippy 0/871(862+9i)/14/
  132/2+2，V2=LF/M1=CRLF 保持。②**技术栈与方法实现细节板块**：
  `docs/TECH_STACK.md` 597 行八子系统（方法/参数出处/实测披露/源码），84 条
  数字→台账证据行，批自纠任务书陈值（径向 d_out 1.4335 已被 √2 取代）；
  README 技术栈节=八子系统摘要挂深度页；官网同名板块八锚深链+实测数字、
  下载区四资产、电池 857→871（4644b04）。③**官网已更新上线**：部署
  26021c0a→别名 autoshop-d7w.pages.dev，25/25 字节校验。④余项见下条。
  M1/V2 快照已刷新至 ledger-snapshots/2026-08-24。
- **🎉 v1.0.0 已发布（2026-08-24，tag `v1.0.0` → `9128cff`，release 四资产
  回下载字节比对全同）**。发版链：W5 批在 8dbcd57 上跑门（clippy 0、
  871(862+9i)/14/132/2+2 隔离根、check_docs 23P0F）→ `cargo build
  --release` 双目标入 dist（CLI 打印 `autoshop 1.0.0`；GUI 不探针）→
  `build_installer.ps1` 无覆盖 → 便携 zip 与安装包同构 27 文件 → 公开文档
  定稿（README 资产表四行+安装/便携两法、发版说明校验表、bug 模板 v1.0.0、
  内部术语 W4/W5/TBD/Deferred 零残留）；沙箱 .git 只读故 commit/tag 由主审
  完成（`release: v1.0.0` 9128cff + 注释 tag），`gh release create` 四资产
  +`--notes-file docs/RELEASE_NOTES_v1.0.0.md`。**资产（主审复算+回下载
  cmp 全同）**：`autoshop.exe` 31,180,152 B `116a3841…68c0`；
  `autoshop-gui.exe` 40,810,704 B `847f42c4…58ce`；
  `Autoshop-Setup-1.0.0.exe` 19,768,387 B `28c4acd3…4750`；
  `autoshop-1.0.0-windows-x64.zip` 27,131,443 B `47389ed4…717e`。硬变更/
  schema 断裂/精度披露全文见本文件尾「v1.0.0 发版义务清单（终稿）」与发版
  说明；M1/V2_PLAN 快照 `~/.claude/plans/autoshop-ledger-snapshots/
  2026-08-24/`。**同日全程**：D2 线性判决失败→H2 机理→修复 ad6de62；W3
  README 53bb77f；官网 autoshop-d7w.pages.dev 上线（8b99111/237b8cc）；
  Inno 183b5b0；W4 8dbcd57。余：自定义域 skymanbp-autoshop.dev 待 zone
  接入、④ GUI 目检（可用安装包）、登记跟进（linear_handle_unwarp_norm 去
  重、R2 大蒙版 ≈1.2pp、图心齐备渲染候选、线性 1px 孤立网格素材、
  check_docs --gates 双配置转录）。
- **🎯 v1.0.0 程序三线落地：W6 官网上线 + Inno 安装脚本 + W4 全文彻查
  （2026-08-24；Codex 额度见顶阵亡一次后续做重派完成）**。**W6**：`site/`
  静态站（8b99111，零 JS/CDN/字体/分析，CSP `script-src 'none'`，含技术栈
  section，资产与 docs/images 字节同源，主审独立验证零问题）；部署走
  CodeEraser 同款 `scripts/deploy_site.js`（母 token 铸 1 小时 Pages Write
  临时 token→wrangler→finally 销毁，237b8cc）；Pages 项目 `autoshop` 经
  API 直建（wrangler create 需 memberships 权限会失败）；**已上线
  `autoshop-d7w.pages.dev`**（首部署预览 1b4e25b2.…），25/25 发布文件与本地
  字节一致、安全头生效；自定义域 skymanbp-autoshop.dev 待 zone 接入。教训：
  母 token 仅 API Tokens Write、/accounts 为空是常态，勿误判「账户未启用
  Pages」。**Inno**（183b5b0）：`installer/autoshop.iss`+`scripts/
  build_installer.ps1`（PS5.1、每用户 x64、27 文件载荷按 config.rs 程序树解
  析/build.rs 内嵌/README 契约核定、权重与测试排除、安全 PATH 任务、卸载留
  develop store、输出 target/installer）；对 0.35.0 dist 实跑产出
  19,761,632 B SHA 98ffa1a1…（主审复算同），安装包未执行。**W4**（本条
  提交）：版本推 1.0.0、电池 871(862+9i)/14/132/2+2、README/ARCHITECTURE/
  V2_PLAN 帧法与 schema 披露对源、`## v1.0.0 发版义务清单（终稿）` 附于本
  文件尾、`docs/RELEASE_NOTES_v1.0.0.md` 英文草稿、check_docs 23→26 行
  （RAW 成员/无预览成员/ARCH-README 电池一致三行，各变异红绿）+`latest_
  published_version` 预发布桥；资产表四格与安装包行留 W5。门主审亲跑见
  提交说明。**下步 W5**：build release→dist→`build_installer.ps1`→tag
  v1.0.0→release（两 exe+安装包+发版说明）→回下载字节比对→README/发版
  说明资产表→消除 W4/W5 内部术语→台账。
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
  在 develop_preview 之后依次调引擎 `geometry_profile` → `apply_lens_geometry`
  (camera/LCP/CA plus manual fallback) → `rotate_straighten`（拉直）完成（导出路径
  同函数、同顺序）。
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
  在数据边界换算，共用引擎 `view_to_original_norm` / `original_to_view_norm`，
  backed by `lens_geom_norm` / `lens_ungeom_norm`; the manual-only fallback
  remains `distort_norm` / `undistort_norm`，全零恒等。完整合约见 render.rs
  "Manual lens distortion" 注释块。
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
3. When a release-sized batch accumulates, propose the next SemVer version appropriate to its compatibility boundary; never hard-code an already-released version here.

## v1.0.0 发版义务清单（终稿，W4 汇编）

> 审计范围=`e75f728..ad6de62`，共 50 笔提交。R30 台账在 v1.0.0 总纲
> （`2bd8167`）下达前写成的所有「v0.36.0 义务」，现统一是 **v1.0.0
> 义务**；历史层积条目保留当时原文，本节是发版与 W5 release page 的现行
> 唯一汇总。

- **R30 B1 报告/安全行为与披露（`ef5a71a`）**：eval 的 n<20 行必须带
  `[low n]`，新增与头条同构的 supplementary n-weighted gap；既有头条、
  state 文件与 n≥20 行定义不变，保障跨版可比。advisor Status 错误体同样受
  house body cap；529 文案按 provider 区分，非 Anthropic 中转的 524/529
  重试均披露潜在双计费；`recovered` 只表示外层响应恢复。XMP census 门与
  store 临时根孪生是发布验证/用户数据安全义务，不改像素或 recipe schema。
- **M-C 评估解释（`2c0f6b4`、`1d00790`、`bba9de0`）**：v0.35.0 的
  17.7% gap 不是 R29 渲染回归证据；同版本重复跑为 16.3%，1.4 pt 摆动与
  跨版本 1.7 pt 同量级。公开材料必须把该结论与低 n 标记一起给出，不能把
  verifier/抽样效应归因给渲染器。
- **R30 B2 现代笔刷载体行为变更（`2cb59a5`）**：读取 sibling `.acr` 的
  content-addressed `MaskBrushTable`，严格核验目录/MD5/信封/Brotli/载荷，
  未观测结构走九类具名拒收；未改蒙版写回时逐字保留原表。现代 Lightroom
  重写 sidecar 的表编码笔刷由 loud refusal 变为导入并按现有笔刷核渲染。
  **零 recipe schema 变化**，但这是必须置顶说明的渲染行为变化。
- **R30 B3 对象蒙版提示与缓存重键（`1e99e84`）**：subtype-0 的 gesture
  `d` 点按原序作为 SAM 正提示，走有界 gp1 JSON prompt IPC；仅「subtype-0
  + gesture」的 alpha cache 加 `gp1` 点列身份并一次性重算。subject、sky、
  无 gesture 的 object 缓存与行为不变；负点、框、权重等未测语义不猜。
- **R30 B4 训练数据/权重披露（`70832eb`）**：ADE20K 与 SA-1B 条款约束
  数据集本身，不自动传递到本仓只下载执行的 OneFormer/SAM 权重；运行时
  权重仍分别按 MIT/Apache-2.0 与既有 digest gate。该批是注释/许可范围真化，
  零运行时与 schema 变化。
- **D1 LINEAR 像素度量渲染硬变更（`ecb6505`）**：非正方帧的斜向线性
  蒙版由归一化坐标度量改为 pixel/aspect metric，实测半等值线误差
  874 px → 9.8 px；轴对齐与正方帧逐字节不变，径向/笔刷不受影响。
- **D2 RADIAL/镜头帧硬变更（`706ac84`）**：Sony 0x7037 原生结半径定为
  `(i+1)/16`，仅蒙版求解边界重采样为 2048 canonical nodes，持久表增至
  64 knots；新增 `LensProfile.mask_warp_center = raw_full_dims/2 −
  DefaultCropOrigin`，`MaskUnwarp` 恰一次组合 `m_lr⁻¹ ∘ T_engine`。径向
  41/41 点向量 ≤1 px（wall 20/20 RMS 0.568 px；第二集 21/21 RMS
  0.243 px），洁净膨胀 ≤0.35 pp、R1≈0.5 pp；**R2 大蒙版约 1.2 pp
  超额仍开放**。普通图像渲染结约定经配准闸维持不动。含相机元数据镜头
  档案的径向配方会重渲染；`mask_warp_center` 是第一处 v1.0.0 硬前向
  schema 断裂。
- **D2 LINEAR H2 硬变更（`ad6de62`）**：`MaskFrame` 三态为
  `WarpedDownstream` / `LinearHandlesToRaw` / `AsRendered`。校正开时线性
  蒙版只在 `T_engine(p)` 的校正帧重建直线；校正关时只把 Zero/Full 两手柄
  一次过 `D_fwd`，再在 raw pixel metric 重建直线，像素环绝不逐点调图。
  新增 `LensProfile.linear_handle_warp`，只在 `DisabledInSidecar` 留存
  LINEAR 手柄图，同时 RADIAL 保持存储坐标恒等；这是第二处 v1.0.0 硬前向
  schema 断裂。含 LINEAR+相机档案的配方在校正开/关两臂均重渲染；中间态
  `706ac84` 从未发布。精度须原样披露为**非 1 px 闭合**：ON
  9.748/7.025/6.336 px RMS，OFF 12.449/9.943/4.979 px RMS；拟合级
  anisotropic-aspect 候选未出货。
- **兼容边界**：v1.0.0 能读取旧配方（两个新字段均有默认值）；旧 exe 对
  `LensProfile` 使用 `deny_unknown_fields`，因此会拒读携带
  `mask_warp_center` 或 `linear_handle_warp` 的 v1.0.0 配方，而不是静默
  丢掉坐标帧事实。W5 的 release notes、README 资产表与发布页必须同时陈述
  这两处 schema 硬断裂、上述重渲染范围、两组精度数字和 R2 开口。
