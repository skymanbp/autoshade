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

- **🎨 色彩质量波·批2 已主审亲核合并 main（2026-08-31 merge `87fc55a`，分支 style-batch2 `a31eb2f`，10 文件 +1655/−80，零冲突）**：G1a=两处撒谎注释改实话（style_pull(1.0)==1.0 全量落靶如实写明）+STYLE_DISTILLED 逐字段点名（{fields} 实测自两份配方 diff，帽 384 字符，蒙版按位置命名绝不用用户文本）；G1b′=蒸馏词汇 12 平面滑杆→五通道（+8 带 HSL 饱和/明度、四色轮 sat/lum/hue、主曲线 [black_lift,s_strength]、逐桶蒙版滑杆幅度；+38 摄入键全派生自 catalogue 无手写表），一致性门 ρ=|mean|/mean(|v|)≥0.75（κ 由四组真实邻居标定：分离带 0.587|0.942 空隙中点；未定义≠通过防整 mixer 置零）、色轮 hue 饱和加权圆均值+浓度 R（算术均值 57.25° 错色实证）、未饱和轮直采目标角、曲线拉形状写回点（夹取保单调）、蒙版按名寻址（批3 加宽时大声红）；style_pull 一字未动＝F1 拉满裁定保持。**亲核实录**：合并树 17 具名亲跑 17P（filtered=1228 旁证总数）、亲手变异「短弧插值改直线」→an_unsaturated_wheel_adopts_the_target_hue_outright 具名红（75P/1F）还原净、电池 **1245(1233+12i)/22/157/2+2** 集差 **+18/−1 逐名**与批报告单一致（−1=承诺收窄的刻意改名）、clippy 0+0、check_docs 27P/0F/0S、字体 864/864（主线超集覆盖批2 需求）、i18n 审计全 0、diff 照片名/密钥 0。批自认 M-B2-F 首活→补 350°/10° 与对色调例后红=覆盖缺口自修。同图 0%/100% 实测：mixer 能量四趟全动（+68.25/+95.50/−22.75/−8.25 随库走非单向）、vibrance 塌缩保持（F1 裁定如实披露）。**登记**：黑白邻居可拖动带目标（检索批议题）、色轮 _sat 单边量方向门恒真、README 风格轴段补五通道（发版批）、style-query 蒸馏预览小批、批2 越界改 advisor/catalogue.rs（+150 行可变镜像，三测试钉，有论证接受）。
- **🍎 Mac M1-M3 已主审亲核合并 main（2026-08-31 merge `14cae27`，分支 mac-port `e124d1a`，34 文件 +2149/−127）**：M1=⌘Q 可移植状态机 quit.rs（busy>答复>未保存，全平台有测）+macos.rs 裸 libobjc 装 applicationShouldTerminate:（零新 crate）+bundled_helper 止于 .app 包+store 平台分店（macOS AutoShade/Library 根、Windows eframe 键按裁定留 Autoshop）+设置面板 Python 解释器字段（Destination 信任、固定候选探测）；M2=python/_device.py 单一 cuda→mps→cpu 阶梯五侧车共用（CUDA argv 逐字节不变）+AUTOSHADE_WEIGHTS_DIR+PYTORCH_ENABLE_MPS_FALLBACK 应用侧注入；M3=build_app_bundle.sh（ad-hoc 签名+plist lint+部署目标交叉核）进 release.yml 带 bundle 内容断言。**亲核实录**：合并树五道电池 **1228(1217+11i)/22/157/2+2** 集差 **+18/−0 逐名**与批自报单完全一致、clippy 0+0、check_docs --gates 27P/0F/0S；亲手变异「source_before_tests 切割规则破坏」→the_verbatim_local_spelling_… 具名红（恰为报告论证依赖切割的那条）还原净；e124d1a 尾行 0。批自报三层根因链（空转源断言→切割位置→重复 #[test] 双注册158→157）经变异复核。三份钉值文档冲突按合并树实测重钉（1216→1228、GUI 151→157，溯源链双环并置）。**用户裁定（2026-08-31）**：v1.2.0 macOS 资产 .app 与独立 CLI zip 双发。**登记**：MPS 时延/内存/deform_conv2d 回退未测（README/TECH_STACK 已按 unmeasured 措辞，测试者回证）；六条别名测试 macOS/Linux 首跑在 CI；M1-10 逐卷 case-fold 与 denoise_cache→weights_dir 字段改名推迟入清账；⌘ 字形与「释/探/系」不在字体子集，措辞绕行（重打字体资产留待有更多缺字时一并）。
- **🎨 色彩质量波·批1 已主审亲核合并 main（2026-08-31 merge `e3d1831`，分支 fit-batch1 `52fb38c`）**：R1=Full 区第三条「严格更优」验收臂（绝对收益越过 MIN_ABS_GAIN=0.012 且帧零回归；zone_accepts 返回具名 ZoneAcceptArm，附着携 ZONE_STRICTLY_BETTER 披露并点名纹理门不算防线）+R2-lite=Atmosphere 参照人口披露（整帧逐通道加权中位数假设+目标侧无置信对应份额，无场读 NOT MEASURED）。**亲核实录**：8 具名测试亲跑 8P（filtered=1207 旁证集差 +8/−0）、岛屿双渲染亲手重哈希同 a406afd0…（361,307,838 B）、模式互斥调用点核实（唯一生产点在 Full 臂）；**亲手变异「Atmosphere 改道 zone_accepts」首绿＝批自认 §5.4 弱点坐实→主审补臂** an_improving_atmosphere_zone_keeps_do_no_harm_and_never_the_absolute_arm（变异上红/还原绿，杀伤验证过）。语料回归 +1/−0（viaduct r1c1：0.045→0.023、帧 −1.4%、纹理 0.936）；p36 恰 0.012 被拒=下限非橡皮图章；**岛屿对诚实闭合 0**（land 臂命中但语义集被 rim 门同基线丢弃；分解=luma 6×/chroma 3× 改善而 wb 3× 劣化，全帧中位数落在天空 0.544-0.566 vs 份额 0.4653，p37 未配对 93%）＝RC-A 测量坐实。**三用户裁定（2026-08-31）**：R2 教义启动独立小批（任务书 quality/task-fit-r2.md，岛屿底三分 R/B_lin 须正向闭合）、MIN_ABS_GAIN 维持 0.012（读法 B：0.024 为锚，读法 A 自我否定）、Mac 资产 .app+CLI zip 双发。合并树五道并行电池 **1216(1205+11i)/22/151/2+2、clippy 0+0**。登记=岛屿 rationale 16KiB 帽挤披露（清账 A2 升级为活例）、denoise 挂钟断言负载抖动（清账 A3 关联）、批更正备忘 §1.1（free-mask zone-refused 非连带，attach.rs:177 仅自身拒绝触发）。
- **🌐 改名 R2 对外段已执行并推送（2026-08-31 `966b337`，主审第一方）**：GitHub 仓库改名 `skymanbp/autoshop`→`skymanbp/autoshade`（旧 URL 自动 301，remote 已切）；仓库内 33 处 slug（32 行）+7 处域名+Pages 项目引用原子换（白名单七文件+deploy_site.js/site README，ROADMAP-archive 1 处冻结台账按类 A 拒改）；Cloudflare Pages 项目 **原地改名成功**（PATCH 直接生效，部署历史保留），pages.dev 子域 `autoshop-d7w` 粘滞不随项目改名——**正式接受为基础设施别名残留**（同 AppId/eframe 存储键一类）；autoshade.dev+www 挂载+DNS 代理 CNAME 建立，apex/www 实测 200，五件字节校验全 MATCH（新 zone 无注入，apex 即可比）；旧域两条 Page Rule 301 实测带路径（/404.html→autoshade.dev/404.html）；archify 架构交付件按新名重生成（validate showcase 0 错 0 警、deliver sha c206059、visual-check 回执与旧回执逐字段同类=纵向溢出已知限制无新故障类），旧名回执六件删除（豁免类 C 归零）、README 舞台裁切图重切 1452×1124；全程 token 铸删（Pages Write+DNS Write+Page Rules Write 三策略一票，值零打印）；diff 照片名/密钥扫 0。**登记小项**：仓库自带 cwd 配置文件仍名 `autoshop.local.json`（每次运行触发别名告警），清账时 git mv 为新名。**在飞**：批1 fit/批2 style/批3 advisor/Mac M1-M3 四个 Opus worktree 并行实现中，完成后逐批主审亲核合并。
- **🏷 改名 R1 仓库内 AutoShade 已合并 main（2026-08-31 merge `e519c1d`：分支 rename-r1 8 提交 `6996b0f`..`8417e3c` + 主审补 `1169c2b`；Opus 子代理实现+主模型亲核，任务书 task-rename-r1.md，报告 r30-materials/rename/r1-report.md）**：105 文件 +3056/−2284；crate/双 bin/66 真变量+占位符/图标文件名/安装器（AppId 与 InstallerStateKey 不变）全换 AutoShade；env 别名单一咽喉 `with_legacy_alias` 三扇门一条策略（34 处直读收编 `live_env`——不接 `resolve_env` 是为不夹带 .env 权限扩张；`trust_of` 先归一化零双表项；每名一次告警；`.env` 双名生效新名压旧名；仅三个测试子进程标记不留别名）；数据目录首启收养四结果（Migrated=同卷整体 rename／KeptBoth=不合并／FellBack=续用旧库／Nothing=不创建）各具名测试，显式 DATA_DIR 永不收养；**四处落盘标识符兼容修 `02dfbb5`＝盲改名会破用户数据**：`x:xmptk` 时代标记（只认新拼法会把全库 era-2 绝对温度重读成 era-1 相对=白平衡漂移；写新读两拼法**永久**、upgrade 从任一 era-1 拼法升当前 era-2；主审废「一版宽限」注释 `1169c2b`）、rationale mark 双拼法、eframe 存储键保 "Autoshop"（%APPDATA% app.ron 是存储键非显示名）、`.autoshop-export-registry` 保留+断言钉死；README/site 的 v1.1.0 下载区与已发资产名保旧（表格描述在架文件）；archify visual-check 回执整套冻结待 R2 重生成。门=电池 1207(1196+11i)/22/151/2+2 全绿、集差 +15/−1 逐名（唯一 − 与 + 成对=xmp 测试更名；+14=6 别名门+5 收养+3 落盘兼容）、clippy 双 0（主审复跑）、check_docs 以**旧名** `AUTOSHOP_CENSUS_ROOT` 跑 27P/0F/0S=别名活体证（主审复跑）、i18n 八项 0（主审复跑）、终扫 305 行/331 处/25 文件逐条归类零未分类（主审复算逐字同）、五变异代理红+主审第六条亲手红（era-2 读取器删旧拼法→红）；渲染零变更=石桥 p36 `match --render --zoned` 以旧名 env 驱动（六告警各一次=别名端到端），TIF 361,307,838 B SHA `b838ba4f` 代理三臂逐字节同、拨盘 63 键同、置信 0.66632223；**主审新确诊（登记事实，非 R1 缺陷）：跨构建像素不同**——CI 发布件同跑 TIF SHA `e37c6ab5` ≠ 本机 `b838ba4f`，但拨盘 63 键跨构建全同（README 教义「same bytes on every run」按运行划界，不违反；配方层跨构建稳定）；勘误=真环境变量 66 非 68（`AUTOSHOP_SESSION_TOKEN__`=HTML 模板占位符、`AUTOSHOP_B3_STAGE_TIMING` 已删只存台账）；R2 交接=32 处 GitHub slug+7 域名+2 Pages 名+archify 重生成+发版说明登记废弃+兼容层退休表（x:xmptk 两拼法永久除外）+两处永久旧名（eframe 存储键/export-registry）各需独立迁移批或正式接受。
- **🚢 v1.1.0 收口发版（2026-08-30，用户令「全自动完成所有剩余任务→网页+文档+readme→推送+发布 release」）**：
  **瓦片边界根修批 `0ecc2e0` 合并 `7328228`**（Opus 子代理实现+主模型亲审，任务书 task-fit-tile-boundary.md）：根因＝rim 尺只读 [0.05,0.95) 权重而硬 0/255 栅格无过渡带→门空过（石桥四瓦全报 0.000/0 transitions，同帧语义门 0.312/492 正常拒）。修＝typed 双尺：软族（语义分割羽化栅格）保原 rim 尺原向量逐字节不变（软族十读数实测 4–745 transitions 非空转、两尺 3/10 翻判 0.40–2.9×＝不可互换，主审裁定不混批）；硬族（瓦片+自由蒙版）改跨边界台阶＝50% 等值线两侧 ±1.5px 配对采样的差分中的差分（对无校正参照帧差分→景物自身边缘抵消）、幅值排序 p90、预算 0.012 不变、零可测穿越即 typed 拒绝（Unmeasured，自由蒙版披露循环补进变量表否则静默丢）；重建守卫双前提+羽化坡对照钉配对采样（一侧化即红），M-A/B/C 三变异红（M-B 主审亲手复红）；校准跨配置守卫由恰好换 2e-4 上界（实测 9.6e-5＝分析仪 cap 4→2 自身代价，此前被空转门下自由蒙版偶然掩盖，推导入注，分析仪 cap 疑点登记 v1.2）；石桥臂 grep unavailable=0、渲染与主树复跑逐字节同一（TIF SHA 432a4cc6 两侧同）
  石桥接缝（无蒙版尺 scripts/rim_overshoot.py）：r0c3 水平 p90 0.0250→0.0052（max 0.0335→0.0106）、最差边 r3c0 垂直 0.0506→0.0176（主审亲测复刻代理数字全同）；look 0.015131→0.017227 在 0.019 全局天花板内、conf 0.6624111 不变；附着 4 瓦+1 场→3 瓦（r0c3 k=0.372@160 穿越、r3c0 k=0.330、r1c2 k=0.773）+2 场。
  **收口件**：xmp 普查钉值按义务刷新 174/40/104/391/1037/40（src/xmp.rs 普查行+邻句 104/104、64/104，recipe.rs 加快照限定语；AUTOSHOP_CENSUS_ROOT 下门 24P/0F/3S）；发版阻断级 CI 红根修 `b10873e`（三处扫源断言写死 CRLF——本机 autocrlf 检出 CRLF、CI 检出 LF（blob 0 CRLF），ubuntu+macos 同挂 style_query 测试且 describe 截断 split 静默失效；统一修＝匹配前归一化，全类 grep 扫过，协议 CRLF 不属此类；本机绿+LF 复刻逐断言真）；docs/RELEASE_NOTES_v1.1.0.md 落地（两硬变更+15 义务条全覆盖，20 引用哈希逐一 merge-base 亲验）；版本 1.0.0→1.1.0（Cargo.toml+lock+README+ARCHITECTURE+bug 模板，门点名后 24P/0F）；合并树统一电池 1193(1182+11i)/22/151/2+2（五道并行）、集差 +2/−0（对 735c959 逐名＝两条边界门测试，主审亲提）；石桥展示图按根修后渲染重拍+重注。发版链=tag v1.1.0→GH Actions（187f361）→六资产回下载 checksums 比对→README/官网资产表实数→issue #2 回帖→官网部署（字节校验走 pages.dev）。收口提交 `f63b795`。
  **发版实录（2026-08-31）**：tag 首指 f63b795 的 run 33345152638 电池全绿但 publish 工位挂＝publish 的 checkout 自带仓库 `assets/`（字体/ICC/图标）与 download-artifact `path: assets` 同名相撞，压平步 `mv` 顶层文件自撞（"are the same file"，bash -e 即死），且消费 glob 会把仓库字体当发布资产上传；修 `7e61288`＝下载路径改 `release-assets/`+压平 `find -mindepth 2`+消费 glob 全改（YAML 步内断言核过）。**电池提速根修 `ac5a924`**（回应用户「这个电池不能并行加速吗？」）：CI 慢三根因＝debug profile（同套件 CI 8677s 对本机 release 288s）+dist 构建与测试零复用+无缓存 → 测试趟全 `--release`+`Swatinem/rust-cache@v2`+`macos-battery` 独立工位与构建并行、共门 publish+ubuntu `debug-asserts` 工位（`cargo test --locked --lib` debug profile）保 debug_assert 面（B1 期该面曾抓 4 红），发版链 2.5h→实测约 25 min。tag 前移 f63b795→7e61288：亲验 `gh release list` 无 v1.1.0 release 对象（零下载暴露）后才删标重打；重跑 run 33353741532 四工位（windows/macos/macos-battery/publish）全 success，六资产上架。**回验**：`gh release download` 后 `sha256sum -c checksums.txt` 五文件全 OK（CLI 19,963,904 B / GUI 26,249,728 B / 安装包 13,923,181 B / win zip 18,696,286 B / mac 通用 zip 36,894,209 B）；runner 产物尺寸与本机构建不同属预期（workflow 内 `--version`+图标校验、电池同源背书）。README 资产表+官网下载区由回验文件实测回填（asset_tables.py 只读下载件，不手抄数字）；官网下载块三处 v1.0.0 残值（两 exe 尺寸、「four assets」计数、macOS 可用性句）同批修正，README/site 残留 `1.0.0` grep 0、diff 照片名/密钥扫 0。官网部署 `6b1b2ef6` 经 autoshop-d7w.pages.dev 字节校验（index/404/styles/robots 四正文+SVG/JPG 两图抽查全 MATCH，其余 31 件 wrangler 内容寻址未变）。issue #2 已被转为 discussion #4，Actions 构建承诺的兑现回帖落 discussioncomment-18212736（run 链接+checksums+复报路径）。
- **🎭 步14 S3 蒙版习惯 + 块预算 + `{e}` 披露门已合并 main 于 `c05c35b`（分支 `style-s3`：S3 `b1d94a5` → `{e}` 门 `b2253aa`；2026-08-30，Opus 子代理实现、主模型亲审收口）**，随后 **发版链 `187f361`**（`.github/workflows/release.yml` 钉 windows-2022 + macos-15、图标告警检查、`scripts/build_portable.ps1` 逐条镜像 `installer/autoshop.iss` 的 [Files]、lipo 通用二进制 + `codesign --force --sign -`、Rosetta 缺席时如实打 `INTEL SLICE BUILT BUT NOT EXECUTED`、发布步产出 `checksums.txt` + 资产表；本机验 30 条目 / 27,745,065 B，兑现 issue #2 的「下版 GH Actions 构建」）与 **三支柱文档 `431c7fb`**（`scripts/pillar_diagrams.py` 手写 SVG 明暗两版、官网 `#pillars` 段、TECH_STACK 新增 `Mask habits (S3)` 小节 + 九处漂移修正）。
  **S3**：`src/mask_habit.rs` 纯函数 `bucket_of()`（AI 子类 2→Sky / 1→Subject / 0→Other；线性渐变按 y-DOWN 哪一端被 `full` 覆盖 XOR `inverted` 定向；径向正立→Subject、反转→Ground；Range Mask 最后判）；`HABIT_SLIDERS[8]` / `HABIT_SLIDERS_SHOWN=3` / `MAX_LOCAL_WORK_CHARS=640`；精修计数同时读配方与 `xmp::MaskImportReason::ForeignRangeMask` 拒绝（169 张 sidecar 库实测 12 拒 / 0 携带）。
  **块预算门**：`advisor::REFERENCE_BUDGET_BYTES=4096` 之下三个消费者（每例看相注、共享标签注、look-reference 块）各自设界——`REFERENCE_DESC_CHARS=200` / `REFERENCE_TAG_PHRASE_CHARS=48` / `REFERENCE_TAGS_CHARS=128`；实测最坏 3,565 B、现实 2,517 B。**主审揪一处假绿**：变异 M-S3-R 首轮活下来，根因是我自己删掉了 join 界断言——块总长探针把两个消费者一起量（实测 168 对上限 128），改成直接对 `block_tags()` 断言后变红。
  **`{e}` 披露门**（`b2253aa`，登记缺陷「侧车错误泄漏路径」根治）：`src/fit_zoned.rs:1427` 的语义交接改走 `crate::rationale::error_line(&e)`，只取首行并剥掉路径，配方 rationale 不再把含用户目录的整段 traceback 写进去；守卫夹具用 `std::env::temp_dir()` 下的绝对路径（首版夹具是裸程序名、错误里根本没有路径，变异证明它**不可能**失败，重建后才成立）。
  门（合并树 `target/style-final3`，release、并行去冗）：**1154 pass / 11 ignored / 1 failed** —— 唯一那条红是 `content_divergence_does_not_fire_on_showcase_same_content`，因为主审换了展示图（见下），不是代码回归；GUI **151/0/0**、clippy 0+0、i18n 0、字体 0；变异 19/19 达标。

- **🖼 展示图重渲（2026-08-30，主审第一方，未提交——用户裁定「改进做完再一起提交」）**：首轮按出厂默认（`--strength 0.65 --style 0.5`）在旧素材上复刻旧版式，用户判「原图、ai分析图、ai生成图、反推图，全都没多少区别」。两条成因都真：素材本身余量小，且 0.65 是**标定保守点**。改法＝换有余量的三张（雾锁湖畔小镇 / 河湾 / 平光石滩）+ 拉到 `--strength 0.9`、style 0 对 style 1.0（唯一变量仍是风格读取）。评分轨迹（全部取自本次转录）：小镇 80→84 采纳、二轮 83 弃 → **Accept**；小镇 style1.0 84→84→91 两轮采纳却仍 **Revise**；河湾 63→69（68 弃）Accept / 68→72→78（72 弃）Revise；石滩 68→78→84（78 弃）Accept / 61→72（64 弃）Revise。反推：小镇 `reimagine` 报 D=0.732、fit 自测 D=0.731 → Atmosphere 模式、look error 0.207→0.093、置信 0.4402213、**零蒙版**；石桥 D=0.126 → Full 解、look error 0.048→**0.015**（全局阶段 0.021，四张冻结证据瓦片买下其余）、置信 0.64636993。两张 neutral 转换与 v0.35.0 已发布件**逐字节同**。`src/fit.rs` 的展示图标定钉子随之重钉（viaduct 0.070→0.126 实测 0.12629355；新增小镇 0.719 实测 0.71865547 作**跨阈**对照臂，测试更名 `content_divergence_is_calibrated_on_every_shipped_showcase_asset`）——**该改动已从主工作树撤回**，与新素材一并在收口时重放（`scratchpad/fit_pin.py`）。旧 lake/sunset 素材不删，移入 SHOWCASE.md 的「早期批次」并标注各自批号。

- **🔬 两处新确诊（2026-08-30 主审取证，均为本机实跑）**：
  **(1) 反推自由度轴在真实内容上是惰性的。** 同一对目标 `--strength` 0.65 vs 1.00：石桥组**逐字段完全相同**（confidence 0.64636993、saturation 38.4、look error 0.048→0.015）；小镇组只有 exposure 1.00→1.07、saturation −9.8→−9.6，色温/色调/曲线不动，look error 不变，confidence 反而 0.4402→0.35（≥0.85 收紧氛围封顶）。`FitBudget` 的边界从来不是约束点，**模式的控制集**才是：D=0.731 落 Atmosphere，而该模式没有 `hsl`/`color_grade`，配方 rationale 自己说了两遍；逐带 HSL 在**任何**模式下都是全零（石桥走 Full 解，`hsl.saturation` 仍 8 个 0）。→ 用户裁定补这条能力，见在飞批 `fit-hsl`。
  **(2) 风格读取传达的是情绪微调，不是看相。** 四个失效点全部有据：CLI 端 `send_reference_image: false`（`src/main.rs:1115`，另有测试钉「非 GUI 面只发文本」`src/main.rs:2610`）→ 模型从没看过参考照片；成片外观库**一直是空的**（六次展示跑全程 `looks: unreachable`，主审已用用户 94 张成片建好，块里随即出现 `warm golden tones / teal-and-orange split tone`，首轮评分 84→86，但渲染仍只是亮度差）；不给 `--guidance` 时两个方向文本项**全程为零**（`txt=0.000(raw=–,raw-fallback)`，W_TXT=4.0 + W_DESC=0.5 完全不参与）；给了方向也在放大噪声（全库 raw cosine 仅 0.899…0.926 约 0.027 跨度，z 标准化后成排序主项，两个语义**相反**的方向 top-1 是同一张）；评委看不见风格（`GradeIntent` 只有 {strength, adherence, direction}，`src/advisor/mod.rs:470-478`），六次跑的修订提示清一色减法，最终全局饱和度落在 +2/−2。量化：style-on 相对 style-off 的位移是 develop 相对中性位移的 52%/68%/95%——**轴不是没动，是动的方向不是看相**。检回来的 RAW 邻居本身就中性（块原话 `THEIR SHARED LOOK … a neutral documentary grade — REPRODUCE this look`），色彩下限算出来是 0（`HSL mixer mean |sat| 2 … wheel saturation 0 … as your FLOOR`）。

- **🧪 步14 收官四批已合并 main（2026-08-31：`ab01520` fit-hsl `78a5ea7` → `13c262e` retrieval-rank `af4ca4c` → `f03b08a` advisor-look `54e2759` → `a6e5a03` stream-fallback `ce3fe42`；前三批 Opus 子代理实现+主模型亲审，第四批主审亲写）**，回应用户两报障「素材库参考没什么用」「反推饱和度色彩没还原」：
  **fit-hsl＝反推逐带 HSL（阶段 4a）**：`hsl.saturation`/`hsl.luminance` 逐 ACR 带按该带自身人口统计（加权均值 chroma / Rec.601 luma）求解，绝不配对像素；准入=既有两侧人口门，拒绝 typed 具名；`hsl.hue` 永不写。闭环走真引擎（2 迭代、±40 步、钳 `FitBudget::hsl_band` 6/18/45）；do-no-harm 两道=阶段自身减半到零（带盲移臂）+ Full 模式后置 4a′ 仲裁对「自身缺席+cast 重拟」重判成品帧（viaduct 实证仲裁承重：仅一道 0.0347、两道 0.0304）。实测 p36 0.032592→0.031792（置信 0.6657→0.6752）、viaduct look 0.052→0.030、校准对 Red/Orange +9/+9 经共享 1e-4 容差上车（**用户裁定：保持容差**——整帧标尺 0.55 权重在亮度 CDF、看不见逐带色彩，是标尺盲区非配方错）。`BandStat` 死字段 `s`/`l`→`c`/`y`（基线零读点，主审逐站亲核）；NumPy 场天花板按文档三步重推 0.0700225→0.0677020（双解法逐顶点 1.1e-5 同意）。变异 12 条：M-D1 三连首绿揪出夹具根因（终端 do-no-harm 先清零致 4a 根本不在测）→换夹具后红；M-D2′ 揭示 4a′ 无合成 Full 钉（语料 viaduct M-D2b 红）与 M-G 带质心 unrepresented 路无测试＝两条登记跟进。
  **retrieval-rank＝文本 hubness 修正 + W_TXT 4→0.5**：任务书前提被实测纠正（0.027 跨度只是四行打印值；全库单方向 cosine 跨 0.073–0.108 不退化）。真根因=hubness（部分样片对所有方向词恒高分：一张样片占某方向 top-4 的 68%、169 库仅 54 张曾被检出）+ 块尺度失衡（d14 5.87 / emb 0.27 / txt 4.00）。修＝z 前减去每候选存量 vocab 均值（估计器与 oracle 偏置相关 0.870；系数 1 两证：OLS 均斜率 0.942、反义 Spearman 在 α=1.00 最小）；全有全无+断言+披露。修正后 (4,4,0.5) 成 CI 排零回归（旧 MAE 优势部分=回归库均值：预测 |z| 0.356 反低于留出照片自身 0.682）→ **用户裁定接受 (4,0.5,0.5)**：MAE 0.688864、+0.024280、CI [+0.005837,+0.041111]；反义 top-1 重合 71%→44.7%、检出 52→149/169、最大占比 59.9%→13.5%；零权重路 bit-for-bit v5。登记：逐方向 OLS 斜率 0.247–2.000（逐方向 β=12 文本上的自由参数，不拟合）、`calibrate_style_retrieval.py` 764 行超预算 14 行、`style-query` 未打印 `txt_hub_corrected` 披露位。
  **advisor-look＝参考图上行+评委看得见风格+色彩下限+蒙版习惯扩容**：B1 `--reference-image`（两单照命令）+ `AUTOSHOP_SEND_REFERENCE_IMAGE`（**Trust::Destination，主审裁定**——决定照片是否上线路；photo-pack `.env` 设不了；batch 不解析）。B2 `GradeIntent::style_look` 携检索看相短语（`look_summary` 与块自身 `shared_look_tags` 同源=评委与提案器永不描述两个看相；512 B 围栏；内联字面量收成单一 `grade_intent()` 使复审轮同覆盖）。B4 `COLOUR_HABIT_FLOOR=5.0`（区间 (2,18] 数据不钉更紧，取提案器自述 ~5..25 下沿）：低于它不再宣称 FLOOR，改引严格正的 `style_colour_floor` 拨盘许可；实测数字永不改写。B5 `HABIT_SLIDERS` 8→10（蒙版内温/色调）+ `MaskHabit::curved`（曲线是点列非滑杆）+ 长度宽容反序列化（S3 期索引仍达版本门的可操作报错）；「蒙版留空」无条件逃逸删除。最坏参考块 3947 B 对 4096 B 门（首版 4108 超 12 B→自削本批文案）；S3 `delta*6` 界重推为 `delta*5`、`MAX_LOCAL_WORK_CHARS` 640→768（**主审裁定接受**，推导入注）；**前向不兼容**：新索引 10 宽 mean，旧 build 读之报 `invalid length 10` 须重建（v1.1 义务表）。13 变异全红（含还原向 pattern 计数抓出 3 处还原失败）。登记：`pipeline.rs` 回退行 `{e}` 插值并入侧车泄漏清扫批。
  **stream-fallback**（主审亲写，见上一条的两层根因）：分支门收口后随本合并入 main。
  **合并树统一电池（五道并行：默认电池‖GUI bin‖clippy 双趟共 target 锁序重叠‖脚本门）**：1191(1180+11i)/22/151/2+2、集差逐名 +26/−0（对 431c7fb 逐名：fit 7/generative 6/style 6/mask_habit 2/pipeline 2/judge 1/openai 1/CLI 1）、clippy 默认+gui `-D warnings` 双 0（钉后增量复核亦 0）、i18n 0、字体 856/856、check_docs `--gates` 26P/0F/1S（census 长期 SKIP；转录 `target/merge-final/release-battery.txt`）。
  **展示图重渲（合并树 CLI，`target/cargo-rel` 深度 3 级）**：小镇 match 拨盘与旧版逐字节同（混色器提案后诚实退回：Orange/Yellow 该对单侧，不可测≠相等；图注补一句披露）→ 资产保留；石桥实变（混色器 Orange −18/+4、Yellow −18/−18、Blue +18/−3 接走 cast 曲线的活，look 0.048→0.015、全局 0.019、瓦片 r0c1/r0c3/r1c3/r3c0 + 1 场蒙版、置信 0.6464→0.6624）→ 重渲 9504×6336 + 重组三联/对照/单幅并三页同步数字；校准钉 `fit_pin.py` 重放（viaduct 0.126、island 0.700 above-threshold 臂、测试改名 `content_divergence_is_calibrated_on_every_shipped_showcase_asset`）后单测绿。**新面板「一张照片三种看相」**（README/site/SHOWCASE 各一）：同帧同 `--style 1.0 --strength 0.9`，仅方向词异，检索各拉回不同成片参考（标签 dark-moody/warm-golden/cool-blue-punchy），渲染实测分离（饱和均值 63/18/75 对中性 45）；评分 92、86→91 采纳、84→73 丢弃，三趟对 direct 档目标皆 Revise 不自动保存。**B1 对照臂已跑未上面板**（golden 方向 ref-image on：87→84 丢弃；两臂仅亮度差 val 144.6/135.9、饱和同——上公开面板会重蹈「效果不明显」，日志归档 showcase3-out/logs；能力以 USER_MANUAL + rationale 双披露为准）。三页照片文件名 grep 0；三看相走直连官方 API（`.env` key 生效，桥密钥被 `file_key_for` 在异端点自动扣留=该防线活体验证）。文档面：ARCHITECTURE 四阶段/氛围混色器/强度表 per-band ±6/18/45/byte-identical 句加时限、TECH_STACK FitBudget 加列+蒙版习惯 10 滑杆+curved+前向不兼容注、USER_MANUAL `--reference-image`+`AUTOSHOP_SEND_REFERENCE_IMAGE`（Destination）+W_TXT 行、README 权重段+hubness+钉表行+三看相面板；电池计数三文档重钉 1191/22/151/2+2。
- **🔌 OAuth 订阅桥实测 + GUI OAuth 模式 Reimagine 缺陷（2026-08-30，用户令「openai 那边的 api 暂时改成使用 oauth 订阅额度」，随后澄清「走订阅只是当前任务」）**：
  桥＝`D:/Projects/_infra/CLIProxyAPI`（v7.2.53，loopback 8317，ChatGPT 订阅 OAuth），设备码登录一次即可（`-codex-device-login`，码 15 分钟过期）。
  **产品早就有这个开关**（用户提醒后核实）：GUI 设置里分析角色 `OAuth (Claude CLI)` / `API (OpenAI-compatible)`（`src/bin/gui/panels/settings.rs:462-463`）、图像角色 `API` / `OAuth (Codex bridge / ChatGPT sub)`（`:551-552`），翻到 OAuth 自动填 `http://127.0.0.1:8317/v1`（`src/config.rs:1627`），帮助文案已写明「capped at ~1.5 MP by the subscription image tier」（`:643`）——**实测桥对任何请求尺寸一律返回 1254×1254（≈1.57 MP），`size` 参数被忽略**，与该文案吻合。
  **实测可用面**：`/v1/responses` 200（提案器 gpt-5.6-sol + 视觉评委），一次完整 `analyze` 端到端 exit 0、judge 82/100、verdict Accept、桥侧六次 200；`/v1/images/edits` 200（上传实测到 20.3 MB、13.9 MB 真照片、stream/quality/input_fidelity 各组合全过）。
  **两处根因（均第一方实证，非推断）**：
  ① 首次接线报 `os error 10054`「远程强制关闭连接」，看着像网络问题，实际是桥侧 **401，4 ms**——`resolve_env`（`src/config.rs:900`）是 `dotenv_map().get(name).or(live)`，**`.env` 的值压过进程环境变量**，所以传给桥的是真 OpenAI key；客户端当时正在上传 13 MB，服务端提前关连接就表现成传输重置。正解＝把桥令牌写进可信设置（`image_api_key` + `image_api_key_base` 钉住端点，`file_key_for` 在端点变回直连时自动扣留它），而不是改 `.env`。
  ② **桥把纯 JSON 正文标成 `Content-Type: text/event-stream`**（实测：正文首字节 `{`，`event:` 行 0 条）。`generative.rs` 原按 content-type 分派 → 走 SSE 读取器 → `image stream ended without a completed event (0 partial(s) received)`，而「2xx 之后绝不重发」的计费纪律使其无法补救。**即 GUI 那个已发布的 OAuth 开关下 Reimagine 根本跑不完**，帮助文案却只说分辨率受限。修法＝声明 SSE 时嗅探正文首个非空白字节（`{` 不可能是 SSE 帧的开头），信正文不信头；`into_json_capped_at` 的 reader 级核心提成 `json_from_reader_capped` 复用，错误种类保留逻辑不复制（`post_ai_json` 靠它区分「回了坏 JSON」与「已 2xx 可能已计费」）。分支 `stream-fallback`，四条具名测试 M-S1..M-S4 + 手工变异。
  杂项：`.ccm/`（cc-memory 插件项目态，用户正把 memory 目录改名 `.ccm`）入 `.gitignore`；远端已核查（fetch 后 `git ls-tree -r origin/main` + 全历史 `--diff-filter=A`）均 0 条 `.ccm`/`memory` 路径，无需清理。
  **第二层（修完 ① 才暴露）**：订阅档按纵横比封顶像素数（实测 3520×2352→1534×1025=1.57 MP，比差 0.02%），被 `canonical_generated_png` 的严格等值契约回绝 → OAuth 模式仍出不了片而帮助文案承诺「仅分辨率受限」。修＝`size_is_an_endpoint_cap`（仅认「双轴皆小+同纵横比 0.5% 容差+长边 ≥1024」，解码尺寸仍须与声明一致），接受时终端+GUI 双面披露实收尺寸。变异 M-S5c「返回请求尺寸」首绿揪出本批自引缺陷：返回串的唯一语义（谈判尺寸，`sent_for` 靠它挑实际上行输入）被第二语义压掉，封顶时保真度 D 量错输入（活体 0.664→修后 0.525）。根治＝拆 `GeneratedImage { bytes, requested, accepted }` 具名字段，M-S5/M-S6 + 变异（容差放宽/删底线/两向互换）全红。活体验收＝桥上 `reimagine` exit 0、封顶披露、1534×1025 落盘。
- **🚧 在飞四批（已全部交付合并，见上条；原派发记录：2026-08-30 派发，Opus 子代理实现、主模型监督亲审；用户四裁定 + 追加第五条）**：
  `fit-hsl`（`target/wt-fit-hsl`，task-fit-hsl.md）＝按人口证据解逐带 HSL 饱和/明度，色相恒零，准入走已有两侧色带证据门，预算随 `FitBudget` 插值，两种模式都接，不得恶化则收缩到零，typed 披露，**硬渲染变更**。
  `advisor-look`（`target/wt-advisor-look`，task-advisor-look.md）＝B1 参考图也能从 CLI 发（默认 OFF、改写那条「非 GUI 只发文本」的钉子测试）+ B2 评委看得见风格（`GradeIntent` 携有界围栏的风格摘要、rubric 增「判它有没有兑现、别因看相扣分」、修订轮也要接）+ B4 风格自己给非零色彩下限（邻居全零时不得再声称 FLOOR）+ B5 蒙版要用得更狠且蒙版内会用色温色调与曲线（复用 `mask_habit.rs`，扩 `HABIT_SLIDERS` 须重算 4096 B 块预算）。
  `retrieval-rank`（`target/wt-retrieval-rank`，task-retrieval-rank.md）＝先量整库 cosine 分布，再在「退化保护降权」与「改打到 `LOOK_VOCAB` 33 词属性上」之间择一，默认（无 embed 无 direction）必须逐字节不变，核心断言＝相反方向检回不同 top-1。
  `stream-fallback`（`target/wt-stream-fallback`，主审亲写）＝上面 ② 的修复；用户裁定「修，并入本批」。

- **🎨 步14 风格检索扩容 S1+S2 已合并 main 于 `74a1e93`（分支 `style-s2`：S1 快照 `2633058`（Codex，中转余额耗尽后冻结）→ S1 修复批 `fc04414`（Opus，task-style-s1-fix.md F-5/6/10/11/12/13/14）→ S2 `ac93b4f`（Opus，task-style-s2.md）→ 夹具 pid `a903068` → 电池去冗 `87f6c04`；2026-08-29/30；用户令 13:00 起 Codex 退役、重读写交 Opus 子代理、主模型监督亲审）**：
  **S1（成片摄入 / 文字嵌入 / 嵌入开关 / 遵循度轴）**：SigLIP 2 图像向量（W_EMB）+ 33 词 LOOK_VOCAB 标签 + 文本塔（`--text-manifest` 批量门；单一 tokenizer 门 `GemmaTokenizerFast.from_pretrained(local_files_only=True)`、`model.text_model(input_ids)`、golden ids + 前向自测）；`EmbeddingSwitch`/`DescribeSwitch`（flag > env > 偏好，**值**而非环境写入）；`score_candidates` 单一评分器 + `DistanceTerms`（`standardise`/`raw_term`/`text_term`）；成片外观库（`MAX_LOOK_EXEMPLARS=500`，只进提示词/参考图，永不进 `style_targets`/`blend_toward`）；遵循度三档 Hint/Direct/Brief 进 verifier；GUI 偏好经 `GradeRequest.embed` 贯通；i18n 乱码根治 + `every_chinese_value_is_real_chinese_and_not_a_console_encoding_accident`。
  **S2（本地描述侧车）**：第五侧车 `python/describe.py`（Qwen/Qwen3-VL-2B-Instruct@89644892，十文件 sha256+字节数经家族单一 `_fetch_verified`；贪心确定性解码；bf16 CUDA 4.05 GiB / ≈1.8 s 每图；提示词 v1 随记录版本化）+ `src/describe.rs`（`parse_records` 拒外来 checkpoint/prompt 版本；`sanitize_desc` 单一门：控制字符/Cf 块按码点、按**字符**截 512；`DescriptionCache` 以帧字节 SHA-256 为键、20,000 条 / 82 MB 上限、原子发布、确定性驱逐；手写 SHA-256 钉 FIPS 180-4 四向量）；建库四阶段各**一次**侧车进程（帧→图像 manifest→描述缓存未命中→文本 manifest）：169 张 RAW **386 s**（S1 逐张重载 5,618 s）、二次 81 s 全命中；`desc_text` 优先散文、标签串回退；参考块 ` · look: <tags> — <desc>`。
  **重标定**（两查询代理，338 向量一批，`target/style-s2/calibration-two-proxy.txt`）：S1 的 (4,0,4) raw 在真散文下 0.698491 劣于无文本 0.695233 → 发布 **W_EMB=4 / W_TXT=4 / W_DESC=0.5 / STANDARDISE_TEXT_TERMS=true**（prose 代理 MAE 0.664818 vs 基线 0.713143，CI [+0.024290,+0.078587]；文本项对本变体无文本行 CI [+0.001589,+0.055436] 排零；变体对决 CI [−0.000205,+0.054341] 险含零；代理＝完美描述，真实 Direction 更短更糙——**用户裁定 2026-08-30 发布实验室赢家，登记「真实短 Direction 复测」小批**）。
  **主审亲核**：S1 修复批报告 708 行逐项（clippy 0/0、定向 232/0/2i、GUI bin 150、CLI 18、M-W21b RED 复证）；S2 报告（定向 245/0/2i、CLI 20、GUI bin 151、clippy 0/0、describe 自测 6/6、八变异复跑 RED→IDENTICAL→GREEN、A/B 索引去描述字段后逐字节同、style-query 六转录 EXIT=0 照片名 0）；`%LOCALAPPDATA%/autoshop` 无残留；`src/fit*.rs` 零 diff。
  **并行电池六红根因**（`a903068`）＝`store.rs` `canonical_temp` 夹具根无 pid，双电池争抢同一 junction 路径 → 加 `-{pid}`，三进程并发 7/0 实证。**电池去冗**（`87f6c04`，用户令「后续电池任务记得优化加速，比如并行、去冗」）＝`gui` 特性只加依赖（`cfg(feature="gui")` 在 bin/gui 外 0 命中）→ gui 趟只跑 `--bin autoshop-gui`；`check_docs --gates` 改按 `Running` 头**按套件名**取数且两趟共跑套件必须相等（五转录形状验证）；CI gui 步同改；clippy 在 cargo 放锁后与测试执行重叠（探针 77 s 藏在 713 s 内）。
  门（合并树 `target/style-final`，release、并行去冗）：**1137=1126 pass+11 ignored / 20 / 151 / 2+2**，clippy 0+0，i18n 0，字体 856/856，check_docs 23P/0F/4S、--gates 26P/0F/1S；集差对 multizone 合并转录 `target/merge-mz/gates-combined.txt` 逐名 **+55/−0**（gui 趟去冗后 lib/CLI/契约套件不在 gui 块重跑，`setdiff-vs-main.txt` 标 NOT RUN；46 lib＝26 style/9 describe/4 pipeline/3 embed/3 advisor/1 recipe，4 CLI，5 GUI）。
  CE 冗余扫（`target/style-final/ce-{dedup,clone}{,-main}.txt`，main vs 合并树）：T1/T2 块 4399→4656、T3 近似对 1406→1599，新增对几乎全为 `(anonymous)` 测试体/闭包（style.rs 内 140 对）+ 侧车家族样板（describe.py↔embed.py/correspond.py 的 die/model_dir/fetch_model/publish）+ `describe.rs`/`embed.rs` `available` 同形 + SHA256_K 常量表——登记为冗余清理批。登记跟进（v1.1 义务条见下）：侧车家族样板重复（CodeEraser 17 区，家族级重构）；描述缓存跨库不 GC；`W_LOOK` 归一化不可测；四个重名 stem；`AUTOSHOP_CENSUS_ROOT` SKIP；`store.rs` 8906 / `check_docs.py` 955 / `i18n.rs` / `ARCHITECTURE.md` 超 CE 750 行门（本批两处根因修以 python 逐字节落地并披露）；staged frames 累积；侧车 `{e}` 路径泄漏；AdherenceTier 命名。下步＝**S3 蒙版习惯**（用户裁定紧接本批，task-style-s3-masks.md）→ 展示图重渲 + 三支柱文档。
- **🗺 步12 多区域语义分区已提交 `a2173c9`（分支 `multizone`，2026-08-29，Codex 实现（中转、worktree `target/wt-multizone`，impl→cont→fix 三会话）→主审只读诊断根因（无条件 thumbnail 上采样）→Codex 修复批第二轮 5h50m 不收敛且反复 `Get-Process cargo,python,autoshop | Stop-Process` 杀全机进程（02:15 四臂/03:44 双电池 exit 127 真凶）→按「两轮不收敛亲自定位」令 03:47 停会话、主审第一方收口；42 代理复审 wf_121078c9 20 确认/16 驳回逐项亲核；合并 main 于 `32b0fe4`）**：
  多类 OneFormer 侧车（`--multi --regions N`，manifest 64 KiB 上限先于解析、平面单一文件名+拒绝链接、平面与 `.tmp` 临时件清扫）→ `semantic::resolve_regions`（两侧过 `MIN_ZONE_SHARE`、置信高→面积小→类号小 优先分配重叠像素，最多 4 区 disjoint）→ 共享 `attach_one_zone` 逐区独立 Full/Atmosphere，置信取最差已接受区（`worst_region_residual`）；`--regions` ≤2（CLI 默认 2；GUI「Up to four semantic regions」默认关、Prefs `zoned_four_regions`）原样走历史天空/地面路由。
  仲裁：四区试跑 vs 同对种子化双区（`fit_recipe_zoned_inner_seeded(.., Some(sky_pair))`，恰两次推理）在**同一把证据尺**上比（`frame_err_under(.., &two.evidence)`——两次全局解可落不同模式，各自 `err_after` 不可比），不优于（含平局 `>=`）即整体退回双区报告 + 一条 `REGION_FRAME_REFUSED{multi,two,regions}`，败方栅格 `release_unselected_rasters` 释放；typed 交接 `SEMANTIC_REGIONS_UNAVAILABLE{e}`（多类层失败：历史路由已跑，不再谎称亮度范围回退）/ `SEMANTIC_REGIONS_NONE{n}`（无区域过门：种子化路由自己判天空分区、删锚点、跑序列器，不再渲染裸 `{s}` 占位符）/ `REGION_BOUNDARY_REFUSED{label,why,…}`（不伪造 after/k）；平面精修按天空/地面同款 MASK_REFINEMENT 披露；manifest `mean_confidence`=质量加权 α（Σp²/Σp）、`share`=均值（原两者同值把覆盖优先级反转），`--self-test` 直接调产品 `rank_candidates`/`plane_stats`。
  根因归并：两座桥共用 `segmentation_input()`（>2048 才 thumbnail；`image::thumbnail` 无 ratio>1 守卫，多类桥原无条件缩略把 1600 px 语料上采到 2048 → 天空平面≠单类蒙版 → 「种子化双区」≠未种子化双区，语料测试连红四轮的最上游）；Rust 层替身侧车贯穿两桥的同一性证伪 `multi_and_single_class_inputs_are_prepared_identically`（300×200 逐字节同、2400×1600→2048×1365 两桥同）。
  主审亲核：六臂拨盘+置信对 662b688 基线 IDENTICAL，rationale 仅多出 typed `ZONE_ALREADY_MATCHED` 句 （neutral-on +3 / neutral-off +2 / raw-on +2 / raw-off +1 / p36 ×2 逐字节同；九臂 EXIT 全 0，活体与电池并发故 wall 不作比较基线）；r4 三臂 neutral-r4 平局 0.092652=0.092652 拒绝（试跑 2 sky ATTACHED / 13 earth ALREADY_MATCHED / 16 mountain ALREADY_MATCHED）、p36-r4 平局 0.023529 拒绝（1 building / 26 sea 被区域边界门 REGION_BOUNDARY_REFUSED、2 sky DROPPED）→两者返回双区结果+一句 typed 拒绝、置信不变（0.2527/0.6621）；raw-r4 保留多区：region-2-sky Atmosphere（D=1.294，EV −0.27）+ region-13-earth Full（D=0.533，EV +0.23，残差 0.047→0.001）、16 mountain 已匹配、置信 0.2730 同双区；三臂 store 无孤儿栅格（refs==disk）；B3 基线 store 已不在，蒙版 sha 比对以拨盘+置信+rationale 句级差替代。门 1058=1047+11i/15/145/2+2 双特性、集差逐名 +24/−0 无状态变化（semantic.rs 11 + segment.rs 6 + fit_zoned.rs 7）、clippy 0+0、i18n 0、字体 848/848（SC 子集重建补「优/侧」等）、check_docs 23P/0F、--gates 26P/0F/1S；变异 M-A/B/C/E2/E3/F/G 红（M-B 反向锚点撞两处→手工修复后全量重跑；M-E 原夹具双方零附着不可观测→释放规则提函数直证）。
  登记＝仲裁一把尺与拒绝分支释放调用点缺确定性夹具（需模式分歧对）；Codex 任务书须明令禁止杀非己进程；Codex 变异台账 M3 引用已删测试、M6 引用够不到该臂的测试（已由新测试取代）。下步＝风格检索扩容。
  合并复核（main+multizone 合并树，`target/merge-mz`，冲突六文件手解：README/ARCHITECTURE/TECH_STACK 钉数取合并实测；`src/main.rs` `Command::Match` 同时携 `regions` 与 F1 `strength`；`src/bin/gui/actions.rs` 走 `fit_recipe_zoned_with_regions(.., FitOptions{strength, provider}, zoned_regions)`；`src/fit_zoned.rs` 多区路由/种子化入口全部改携 `fit::FitOptions` 而非裸 provider，升格站点改用 `fit_recipe_from_promoted_with_disclosure_opts`；semantic.rs 测试改 `FitOptions::default()`）：1091=1080 pass+11 ignored/16/146/2+2 双特性、集差对线性落差合并 `6323f4c` 转录（`target/merge-lin/gates-combined.txt`）逐名 +24/−0、clippy 0+0、i18n 0、字体 848/848、check_docs 23P/0F、--gates 26P/0F/1S；九臂（合并 CLI，显式侧车路径）：六默认臂拨盘+置信对 F1 期 `target/f1-review/arms`（`302efb1` 树；其后 cleanup-17/线性落差不触反推路径）逐字节同、rationale 仅 +3/+2/+0/+0/+0/+0 句 typed `ZONE_ALREADY_MATCHED`（neutral-on land+r0c0+r1c0 / raw-on land+r1c0 / p36 两臂零差）；三 r4 臂 neutral-r4 / raw-r4 / p36-r4 拨盘+置信+rationale 与分支 `mz-live-final` 逐句同、store refs==disk 1/2/3 无孤儿。主审揪合并臂夹具一坑：合并 CLI 建在 `target/merge-mz/cargo-cli/release/`（比 `target/release` 深一层），`bundled_helper` 三级祖先搜不到 `python/`，首轮「on」臂全部静默走亮度范围回退（rationale 「Zoned sky fit unavailable」）——非代码回归，显式 `AUTOSHOP_SEGMENT_SCRIPT`/`AUTOSHOP_CORRESPOND_SCRIPT` 重跑六臂后比对；教训＝活体臂 exe 深度必须与 `target/release` 同级或显式给侧车路径。
- **📐 步15 线性落差翻转 `LINEAR_FALLOFF = Eased` 已提交 `817fa13`（分支 `linear-falloff`，2026-08-28，Codex 实现（中转、worktree `target/wt-linear`）→主审亲核三处亲改；合并 main 于 `9547f36`）**：
  **测量（主审第一方，用户手工在 Lightroom 拉同几何渐变导出 16 位 TIFF `probe-lightroom.tif`，行噪声 0）**：`scripts/linear_falloff_probe.py --compare` Autoshop Clamped 两端转折 0/3 行 vs Lightroom 80/80 行；`--fit`（本批新增：手柄行与剖面联合拟合，残差含两侧平台，软剖面无法靠缩短跨度伪装成线性）Lightroom 对 smoothstep rms **0.0045**（sin 0.0044 同族）vs 线性 **0.0169**，手柄回收 697/1678；Clamped 渲染回收线性 0.0003（700/1599）、Eased 渲染回收 smoothstep 0.0002（700/1599）＝两把尺互证。**首版报给用户的 0.029 vs 0.049 是 1%/99% 端点自检下的数字——端点自检在软剖面上会切掉跨度（smoothstep 在 t=0.082/0.918 才过 2%），差距被低估 2×，文档改钉自由端点数字。** 用户 19:05 裁定翻 Eased（v1.1 渲染硬变更，仅线性蒙版；径向/位图逐字节不动；`RELEASE_NOTES` 段落已备于 `target/linear-falloff/flip-report.md`，发版批再入文件）。
  代码：`src/render.rs:47` 一行翻常量，`linear_coverage` 体/手柄输运/MaskFrame/XMP schema 零改动；六处斜坡派生钉值按同一法则重钉（t=1/3,2/3 → 66/189；t=0.025/0.525/0.975 → 0/137/255；顶行 1/8 → 0.04296875；dehaze 边界列 31 t=1/64 → 7.3e-4 低于 0.001 工作地板故并入不变平台）；`radial_linear_bitmap_masks_match_the_clamped_baseline` 拆三：径向/位图各自逐字节同 Clamped 基线、`linear_mask_renders_the_eased_ramp`（eased 逐字节 + 内部与 Clamped 必异 + 四端点相同）。
  **主审亲改三处**：①ARCHITECTURE 计数句被 Codex 改史（B3 句头换成 flip、尾巴留 B3）→ 还原为追加式 1034→1041（+7，`56dd690`）→1044（+3）；②`mask_coverage_reports_the_engine_weight` 注释被改成断句 → 重写并说明 Eased 下行 0 不再区分两种采样约定、行 10/19 接任；③端到端证伪 `shipped_linear_ramp_is_eased_end_to_end` Codex 版用 8 位预览+「平台 vs 斜坡平均斜率」当跳变（任何剖面都非零，Codex 变异 (c) 只靠中点比值抓住，报告却称「检查真实一阶差分」）→ 重写在 f32 路径：色调通道 `p·(1−w)+t·w` 使平灰上覆盖度可精确回收 `(base−row)/(base−full_plateau)`，逐行钉**字面** 3t²−2t³（容差 2e-3，预言不引用 `linear_coverage`）、两手柄内 10 行斜率 ≤0.1×中段、中点 1.5×线性；主审变异 M-a（常量回 Clamped）/M-b（Eased 臂返回 t）/M-c（返回 t²）全红，M-c 现由端到端公式钉 :14725 抓住。
  门（worktree 内 `CARGO_TARGET_DIR`）：默认 1033+11i / 15 / 2+2，GUI 1033+11i / 15 / 145 / 2+2，clippy 0+0；集差对 `target/b3-main/gates-final.txt` +10/−0（56dd690 的 7 之中 5 名存活 + 本批 5 新名；改名去 `shipped_linear_falloff_is_clamped`、`radial_linear_bitmap_masks_match_the_clamped_baseline`）；check_docs 23P/0F/4S、--gates 26P/0F/1S；i18n 0；字体 847/847。
  文档：README What-is-new 条、ARCHITECTURE §掩模、TECH_STACK、USER_MANUAL 线性渐变句；数字全部来自 `--fit` 转录。Codex 首批遗留仓库根 `target-report/report.md` 移至 `target/linear-falloff/first-batch/`。
  暂存扫描：`_?DSC[0-9]{4,5}` 0、密钥 0、用户目录路径 0。
  合并复核（main+linear-falloff 合并树，target/merge-lin）：1067=1056 pass+11 ignored/16/146/2+2 双特性、集差对 cleanup-17 合并 43d3bcb 转录 +10/−0 逐名（render::tests 十条）、clippy 0+0、i18n 0、字体 847/847、check_docs --gates 26P/0F/1S；三处文档计数冲突按追加式解，render.rs 自动合并且 LINEAR_FALLOFF = Eased 保留。
- **📐 步15 线性落差 C¹ 测量线已提交 `56dd690`（分支 `linear-falloff`，2026-08-28，Codex 实现（中转、worktree `target/wt-linear`）→主审亲核；合并 main 于 `9547f36`）**：
  单一 `linear_coverage(t, profile)` 取代线性臂三处内联夹紧斜坡（`mask_weight`×1、`mask_weight_metric`×2）；手柄输运/MaskFrame 法则/径向与位图采样/XMP schema 零变化；
  `LINEAR_FALLOFF = Clamped` 出货（渲染逐字节同 HEAD），`Eased`＝Hermite smoothstep 3t²−2t³ 休眠——翻常量（`src/render.rs:47`）须等 Lightroom 探针测量，属 v1.1 渲染硬变更义务，本批不动、RELEASE_NOTES 不动。
  测量夹具：`scripts/linear_falloff_probe.py`（自足、16 位、`--compare a.tif b.tif`）+ `probe_fixture_round_trips_through_xmp` 在 `AUTOSHOP_GENERATE_LINEAR_PROBE=1` 下经项目自己的 XMP 写手生成
  `target/linear-falloff/probe/`（0.18 灰 3000×2000、单竖向渐变 zero y=0.80 / full y=0.35、−2.00 EV）；Autoshop「前」值：斜坡 2.5174e-4/行、两端跳变 2.5174e-4、full 端转折 0 行（硬角）、zero 端 3 行；`操作步骤.md` 已交用户，回「好了」后主审跑 `--compare`。
  测试 +7/−0（按名对 `target/b3-main/gates-final.txt`）：clamped 位级同 HEAD、eased 两端 C¹、两剖面手柄处一致、单一定义源断言、出货常量钉 Clamped、径向/线性/位图对 clamped 基线逐字节、探针 XMP 往返。
  **主审揪一缺口**：手工变异 M-L2（删 `[0,1]` 夹紧）首绿——测试用 `as u16` 饱和转换比较，负值与 >1 覆盖都饱和成与 head 斜坡相同的 0/65535 → 改 f32 `to_bits` 精确相等后红（exit 101）；M-L1（Clamped 臂返回 t²）红 ×2；Codex 四变异红。
  门（worktree 内 `CARGO_TARGET_DIR`）：默认 1030+11i / 15 / 2+2，GUI 145，clippy 0+0，check_docs 23P；首跑 1 红＝`denoise::tests::a_stalled_sidecar_is_killed_and_its_claim_is_released` 的 `elapsed < 5 s` 挂钟断言在四路并行构建下超时（单跑 3× 0.25 s），登记为收口线观察项（计时断言应量机制而非挂钟）。
  暂存扫描：`_?DSC[0-9]{4,5}` 0、密钥 0、用户目录路径 0。
- **🧹 步18 收口小批（计划编号按 cc-memory 现行 18 步：15 线性落差 / 16 改名 / 17 展示图 / 18 收口；本台账更早条目里的「步 15 展示图」「步 16 收口」是插入线性与改名两步前的旧编号）（侧车错误披露泄漏 / xmp.rs 注释漂移 / 发布电池 fs 抖动）已提交 `9097319`（分支 `cleanup-17`，2026-08-28，Codex 实现（中转、worktree `target/wt-cleanup`）→主审亲核；合并 main 于 `43d3bcb`）**：
  A. `rationale::error_line(&anyhow::Error)` 单一助手：取顶层错误首行、绝对路径（盘符/Unix）缩为基名且用户目录段（`Users/<x>`、`home/<x>`）整体折成 `[path]`、错误链带退出码时追加 `; exit N`、≤160 字符；`("e", …)` 类披露 14 处全部改道（fit / fit_zoned / pipeline / retouch / advisor::heuristic：correspondence、zoned、revision/verify、style、judge、heuristic、heal），stderr/日志仍保留全文；具名测试 `sidecar_failure_disclosure_has_no_traceback_or_home_path` 覆盖助手 + 12 个 rationale 键渲染；**主审改进**：首行为 `Traceback (most recent call last):` 时披露改取最后一行（异常消息，如 `ValueError: boom`）而非笼统 "operation failed"，并补证伪断言（原分支无测试覆盖，手工变异 M-17-B 首绿→补后红）。
  B. `src/xmp.rs` 注释普查数字对齐钉住的普查行（177 sidecars / 42 Aggregate / 105 Mask/Image / 398 Paint / 1081 Mask/* / 40 Gesture）：218→105、83→40、135/218→65/105（＝105−40，Gesture 是 Mask/Image 唯一可选子元素），ARCHITECTURE 两句同步；主审第一方复核当前库 `D:/Photography/Raw`：174 文件 / 104 Image / 64 自闭合 / 40 Gesture——**登记观察项**：钉住普查相对当前库已漂移 3 文件，v1.1 发版设 `AUTOSHOP_CENSUS_ROOT` 时刷新钉值。
  C. 发布电池 fs 抖动：Codex 同 worktree 5×pipeline+5×store+build 并发压测未复现故未改；**主审在三 worktree 电池并行时第一方撞上**（`store::tests::a_zero_byte_live_claim_yields_to_the_surviving_bak` `Os error 5 拒绝访问` store.rs:6095）→ 根因＝测试夹具目录用固定名 `temp_dir().join("autoshop-…")`（无进程 id），跨进程互删互读；修前压测（同一测试二进制三进程各 12 趟）**29/36 失败**→ 一次性全类改名 78 站点（store 49 / pipeline 12 / gui tests 7 / decode 3 / render 3 / serve 2 / main 1 / retouch 1：名字加 `-{pid}`，扩展名前插入，各测试自己的建删仪式不动；拒绝两处类外：`store.rs:420` 生产回退、`pipeline.rs:4433` 不存在路径探针）+ 源断言 `fixture_dir_tests::test_fixture_dirs_are_process_unique` 递归扫 `src/**/*.rs` 钉零固定名 → 修后压测 **0/36 失败**（同三进程×12 趟），断言+store+pipeline 套件 142 绿。
  门（worktree 内 `CARGO_TARGET_DIR`）：默认 1036（1025+11i）/ 15 / 2+2，GUI 145，clippy 0+0，集差 +2/−0（`rationale::tests::sidecar_failure_disclosure_has_no_traceback_or_home_path`、`fixture_dir_tests::test_fixture_dirs_are_process_unique`），check_docs 23P/0F/4S；主审变异 M-17-A（基名保留整段路径）红、M-17-B（Traceback 分支旁路）补测后红、M-17-C（永不追加 exit）红、M-17-D（重新引入一个固定夹具名）红（源断言指出 store.rs:6083）；Codex 三变异红。
  主审揪一处文档改史：ARCHITECTURE 电池计数句被改成「B3 批 1017→1035」→ 还原为追加式（cleanup 1034→1035 +1/−0 对 662b688；B3 1017→1034 保留）。
  暂存扫描：`_?DSC[0-9]{4,5}` 0、密钥 0、用户目录路径 0（`src/rationale.rs` 测试里的 `C:\Users\alice` 为合成夹具，非本机路径）。
  合并复核（main+cleanup-17 合并树，target/merge-c17）：1057=1046 pass+11 ignored/16/146/2+2 双特性、集差对 F1 302efb1 转录 +2/−0 逐名、clippy 0+0、i18n 0、字体 847/847、check_docs --gates 26P/0F/1S；三处文档计数冲突按追加式解。
- **🎚 步11 F1 自由度轴已提交 `302efb1`（2026-08-29，用户 2026-08-28 裁定强度轴支配反推诚实预算 + style 1.0 拉满；Codex 实现→七路 Codex 只读复审 42 发现主审亲核→修复批三（根因归并 F-A…F-H）→主审亲核再修六处收口）**：
  `FitBudget::for_strength` 三点线性插值（0.00 / 0.65 默认 / 1.00）：EV ±0.5/±1/±2.5、饱和 ±15/±30/±60、WB 增益 [0.90,1.12]/[0.80,1.25]/[0.50,2.00]、增益比 1.20/1.40/3.0、曲线斜率 [0.7,1.3]/[0.5,1.5]/[0.25,3.0]、Full 色偏比 1.5/2.0/3.0、置信帽 0.50/0.50/0.35（0.85 处 0.414）、WB 旋转份额默认以下 0.05、之上线性开到 1.0（0.85 处 0.593）；≥0.85 否决改披露（`FIT_NOTE_VETO_DISCLOSED` + 置信封顶）。
  默认 0.65 逐字节不变（WB 含在内：超预算=as-shot），唯一裁定例外＝方向一致的全局色偏在每档强度都可测（`global_cast_is_measured_when_every_band_is_one_sided_and_consistent`）；默认以上 WB 沿拟合的 log-K/线性 tint 流形 λ 二分收缩（采样合法端点保证、恰一次圆整为渲染器 WB），前后渲染须过外来色相否决 + 加权旋转预算，否则 as-shot 并 typed 披露（`FIT_NOTE_WB_WITHHELD_FOREIGN_HUE` / `_ROTATION` / `WB_CLAMPED{from,to,rotated_share,coverage}` / `WB_SEARCH_BOUND{k}` / `CAST_ADMITTED_BY_STRENGTH{ratio,budget}` / `STRENGTH{pct,s}`）；rescoring 回读 `s` 四位小数、各 SolveFacts 皆携 `Some(budget)`。
  Style 轴：`render_reference_for_style` 在 Style≥0.85 改「TARGET style to reproduce」措辞（非粗体字面量逐字节钉 HEAD）、`style_pull`（0.3→0.18 保持出厂，1.0 拉满）取代 0.6 帽；CLI `match --strength`、GUI 面板 Strength 经 `panel_strength()` 单一读数接入反推。
  普查停机：书中 F-B「色度不可测像素以亮度权重回退」在六臂重证前被两条既有测试拦下（雾霾对 0.0547→0.0201＝0.367× 未达 <0.35×；真实峡谷色偏在 [Red, Blue] 色相证据下被扣留而非重建）→ 按停机规则撤回，普查保持色相证据单一原语（Full 色偏门与默认以上 WB 门共用），盲区以 `{coverage}` 披露；用户 02:00 AskUserQuestion 答「依旧，优雅、高质量。怎么效果最好怎么来」→主审裁定撤回（两证据皆指回退更差），盲区 {coverage} 披露即交付。
  主审亲核：六臂（neutral/RAW/p36 × on/off）拨盘+置信+rationale 对真 B3 基线 IDENTICAL（cmp6.py 基名规范化；蒙版栅格 sha 全同，`-4/-5/-6` 为库内 claim 去重后缀）；集差逐名 +22/−0 默认、+23/−0 GUI 对 662b688 转录（BOM 感知解码）；展示三对九档 3000px 渲染 EXIT 0（云南湖船 1.0＝2600 K/tint −91.5/EV −2.5 深蓝对齐重绘目标、康沃尔 1.0 撞 40000 K 搜索上界并披露）。
  主审揪出并亲修六处：①GUI 测试同义反复→`AutoshopApp::panel_strength()` 单一读数 + include_str 接线钉（变异 M-G1 红）；②`rescore_report` 丢弃高强度否决披露与预算（Full 模式 budget None）→ 每模式 `Some(budget)`、披露按当前配方重推导（严格教义无担保）+ `rescoring_re_derives_the_high_strength_veto_disclosure_and_its_cap`（变异 M-R1 红）；③ARCHITECTURE 计数句自相矛盾（1034→1054 接 B3 +17 尾巴）；④门 5 表行仍写「committed band」；⑤`match --strength` 帮助文本抄自 analyze；⑥WB 搜索域 2000/40000 字面量三处→`WB_SEARCH_K`。Codex 七变异红（fix3-mutations.md）。
  门 1055=1044 pass+11 ignored(+11i)/16/146/2+2 双特性、clippy 0+0、i18n 0、字体 847/847、check_docs --gates 26P/0F/1S；报告 target/f1-freedom/fix3-report.md、活体 target/f1-live3/、主审门 target/f1-review/。
  登记＝元根因：无「默认快照黄金测试」让两轮默认漂移只被六臂活体抓到（收口步补）；`AUTOSHOP_*` 别名折叠待收口；src 注释 19 处历史文件名待用户裁定别名化。下步＝四线合并（F1→cleanup-17→linear-falloff→multizone）。
- **🧮 步10-B3 余量自由蒙版已提交 `662b688`（2026-08-28，Codex 实现→Codex 只读复审 5 MAJOR + 五视角代理复审 wf_ca8c0e99 判 FIX-THEN-SHIP→Codex 修复批（中转）→主审亲核收口；用户令：此后对抗复审 fan-out 改派多路 Codex）**：
  局部场之后、瓦片之后的第四生产者读余量：候选像素＝weight>0 ∧ |remainder|>2/255 ∧ 已接受瓦片 alpha<0.5；4-连通同号分量按 Σ|remainder|×weight 排名（seed 决胜＝确定性），
  64 像素底线→双方 scoped 证据份额≥3%→D<0.65→帽 2；每个分量必有 PROPOSED / ATTACHED / REFUSED{footprint, mass, share, divergence, cap, raster-claim, raster-write, zone-refused, frame, rim} 之一，无候选写 NONE；
  附着＝最近邻上采样 2048 + 半径 8 引导精修 + ZoneAttachment(Custom/Bitmap) + 与瓦片共用一条 `enforce_bitmap_boundary`（帧零容差 + rim 0.012，Frame/Rim 分型）；拒绝只回滚试探性附着注、保留精修注；停机披露 `skipped` 按层开关派生。
  修复批闭合复审五项：A 关层逐字节（stop 前/后/不停三分支）；B 拒绝披露保留 + Footprint/RasterWrite 拆分；C 反证补齐（不等质量反号、4-连通、帽、真瓦片排除、typed 拒绝/附着、秩确定性）；D 文档 + check_docs 新增 TECH_STACK 电池 Claim；E `AUTOSHOP_B3_STAGE_TIMING` 删、`propose_free_masks` cfg(test)。
  主审亲核：四臂配方 compare_recipes 对 B2 supervisor 全 IDENTICAL（拨盘+置信）；六臂 EXIT=0、**无一自由蒙版附着**（neutral 提案 1−/2+ 被 zone-refused/rim 拒，RAW 1−/5+，p36 1+/2− 同）；像素尺 ON 12.49/20.58/5.89、OFF 10.63/16.46/5.87；rationale 11,645/9,282/15,998 B。
  主审揪出并修：Codex 文档三处（ROADMAP M-表被注释断表 + `\r` 误改→还原；ARCHITECTURE 计数注解 1017/1021 自相矛盾且引本机 target/ 路径；README 两句错位→并为 §6 段落）；手工变异三条：M-B 红（帧/rim 拒绝不计入结构化结果→语料双红），**M-A / M-C 绿＝覆盖缺口**（自由蒙版阶段后的 realized 披露无人钉；真实 zone-refused 路径的 typed 拒绝只被 cfg(test) 捷径覆盖）→补 `free_mask_stage_publishes_its_own_realized_reading` + `free_mask_real_zone_refusal_is_typed`（ZONE_ALREADY_MATCHED 真路径）后复跑均红；Codex 十变异红（mutations.md）。
  门 1034=1023 pass+11 ignored(+11i)/15/145/2+2 双特性、集差 +17/−0 逐名对 d21304a、clippy 0+0、i18n 0、字体 847/847、check_docs 23P/0F/4S、--gates 26P/0F/1S（主审转录 target/b3-main/gates-final.txt）。
  同批无代码文档提交：`08e376b` What is new + 两批六帧结果、`d3ed3a0` 删两句提前入 README 的 B3 句、`5bdc202` What is new 按算法重写、`471f15e` archify 图加创新组件、`ada3c68` README 精简（991→896 行，token 集差核对信息不丢）、`a2d6731` 手册拆到 docs/USER_MANUAL.md（243 行；SECURITY/site 链接同步）。
  登记观察＝瓦片排除用原始 alpha（rim 收缩后的瓦片仍挡提案）、64 像素底线是瓦片无底线的回退、语料上全被下游拒＝本批交付披露非修正、p36 OFF 继承转录 16,384。下步＝F1 自由度轴（用户 12:00 裁定，任务书 task-fit-freedom.md）。
- **📘 README 九段重构 + archify 架构图已提交 `2ea6c7e`（2026-08-28，用户令：读全局 README 标准；before/after 恰放三对；用 archify 出整体架构图）**：
  README 按九段标准重排＝What Autoshop is（简介+受众+教义）/ What it does（Feature 1–8）/ How it works（archify 架构图 + 主路径 + 七条「难在哪」）/
  Results: before and after（恰三对：①AI 分析＝猫对；②带风格读的 AI 分析＝湖与船 neutral vs 四参考被接受的 style05；③AI 整图+反推＝高架桥 gpt-image-2 3520×2352 目标 vs
  反推配方在 RAW 9504×6336 渲染，0.057→0.019/0.678264）/ Measured numbers（13 行，逐行引自持有该数的段落，无新数）/ Install / User manual / Supported formats /
  Tech stack, algorithms, and design philosophy（新增六条哲学 + 原技术栈正文原样）/ Status, roadmap, and known limitations / License。
  逐行集合比对：旧 742 行中 23 行未原样保留＝目录重生成、Feature overview 六条改写为 Feature 1–8（原六项全覆盖）、两个标题改名、SHOWCASE 内 hero 句改指向；
  Part A/B（三旧对含两处失败模式、两张风格三联、两张反推三联）原样迁 docs/SHOWCASE.md（图径改 images/）。站点 #install-and-quickstart 锚点保留；站点展示按
  showcase-replace-pending 待步15 一并重部署，本批未动 site/。
  两张新对图 docs/images/showcase-lake-style-pair.jpg / showcase-viaduct-reimagine-fit-pair.jpg 由 r30-materials/showcase2-out/full 全尺寸渲染合成（1600×594，
  猫对版式，无 EXIF；scratchpad compose_pairs.py）。架构图＝docs/architecture/autoshop.architecture.json（archify `validate --quality showcase` 0 错 0 警）+
  autoshop.architecture.html（deliver）+ docs/images/architecture-{light,dark}.png（README `<picture>` 按明暗切换）。首稿由 Opus 5 代理起草，代理撞会话额度后主模型亲手收口：viewBox 2110→1270 分层版式修桌面可读性、六处标签偏移修重叠、截断标签改短；
  visual-check 可读性/查看器铬通过，**四桌面尺寸纵向溢出登记为已知限制**（卡片区在折叠线下，scrollHeight 1430/1538/1538/1565）；README PNG＝2048×1320 截图舞台区裁切 1452×1132（sha 前缀 json af218bba/html 370970d0）。
  门：check_docs 23P/0F/3S、--gates（B2 合并转录）25P/0F/1S、README+SHOWCASE 照片文件名扫描 0。
- **🧮 步10-B2 局部场分析仪已提交 `d21304a`（2026-08-28，Codex 两度撞额度→阶段 A 主模型/Opus 5 亲写、阶段 B Codex 起头+Opus 5 续跑门/活体/文档、主模型主审+53 代理对抗复审后一次系统修）**：
  `src/fit_field.rs` 只读 12×8×8 双边网格 ×5 参数（ev/gain_rgb/slope）CG 解（f64 累加、λ=1 Tikhonov + s=1 拉普拉斯、≤90 迭、rr≤1e-10、三线性 splat、
  界 EV±1.25/增益±0.35/斜率±0.5、占据下限 8），拟合权=冻结证据 source_weights × 局部支持 (1−D，96 格 rayon 并行按格序散布=两次求解逐位同) × 未裁剪；
  输出 ceiling/global（报告自己的尺）、带边际/带离散度/余量+逐像素拟合权/饱和数；场永不进 render/recipe/xmp（grep-pin）。NumPy 交叉验证（scripts/field_check.py +
  grid_experiment.py + prepare_pairs.py 自 fitgrid/ 迁入，实验产物与探针 crate 存 r30-materials/fitgrid-experiment）：ceiling 0.070022 两侧同、768 顶点最大差 1.5e-5。
  `fit_zoned/field.rs` 读判：BAND_DISPERSION_MAX=15/255（阶段 A 扫描：均匀 ≤9.2、结构 21.9–51.8、校准中间调 28.7–29.1 /255）、bin 0 按构造盲（`0:blind` 只报一次）、
  加权 R² 瓦片/线性（平面先得 linear 后瓦片改增量，故 linear 必带帽 2）、有效瓦片帽 4→2、带提案=当前渲染亮度跨度（非证据 bin 索引）；序列器持有
  realized=(global−err_after)/(global−ceiling) 与 LOCAL_STOP（≤0.002 且 ceiling<global 才跳过瓦片并点名）；range 并集点 `evidence_bins_for_span` 经占据像素的原始亮度
  10–90% 分位把跨度映射到证据 bin（−1 EV 全局后两域差约两 bin）、反号重叠/自符号分歧两种典型弃权、同号并入带 `{why}`、仍在四带帽之前；attach_tiles 帽参数化；
  五键 LOCAL_CEILING/SHAPE/BAND_SKIPPED/REALIZED/STOP + RANGE_MERGED {why} 双语（字体 +场/散/迭/顶 847/847）。
  **主审+复审修**：Codex 的 clear_finished_disclosure 从 MAX_NOTES 有界 vec 重建 rationale 会截断→删；LOCAL_CEILING 硬写 realized 0.000→实测（校准钉 ==0.000=两尺同一）；
  复审 9 项确认全修＝停机无守卫、提案域错配、自符号未校验、合并注措辞假、帽无直接断言、余量混入占据置零顶点、并集测试空洞、禁用层同义反复、文本 grep 式帽测试→
  行为式（cap 0 附 0 瓦）；语料测试钉局部支持非常量（合成夹具 ≤144×96 与 >5.4:1 / <1:4 画幅下支持项退化为 1.0 已登记）。
  **活体**（修复后 exe 5cfb5e6c vs `autoshop-pre-b2.exe`，四臂各自店）：neutral 开 0.189→0.093、关→0.080、RAW 开 0.194→0.098、关→0.108，四臂拨盘+置信归一化
  SHA 全同（仅 masks[].mask.path 随店）、apply 渲染逐字节同（7d58d379…/7da778ca…）、像素尺 12.49/20.58/5.89（开）10.63/16.46/5.87（关）不变；披露 global
  0.096145/ceiling 0.070022/饱和 3/CG 39、R² 瓦片 0.394 线性 0.050 free_form 帽 2、结构 bin [0:blind,3,4] 29.14/28.72、天空区 realized 0.134、范围带 0.620；
  RAW global 0.107764/ceiling 0.072299/R² 0.331/0.039、天空区 0.280、关 0.000；旗舰对无提案存活（3/4 结构、余者份额/2/255 线），并集未动任何带；LOCAL_STOP
  活体未触发（余 ≥0.0099）单测钉；ZONE_DROPPED 漂移 `{:+.5}` 令 r2c0 +0.00036/+0.00035 不再打成零；rationale +544 B（10331→10875 <16 KiB）；分析仪成本 solve 1.343 s。
  门 1006(+11i)/15/145/2+2 双特性、clippy 0+0、i18n 0、check_docs 23P/--gates 25P、集差 +26/−0 逐名；变异 12 红（Codex/Opus A–G + 主审 stop-guard/own-sign/
  domain-map/union-loop/shape-weight）。复审驳回项（上界尺偏置、离散度单地、外扩取整、停机时序、cfg_attr 死码）与登记项（提案被筛无披露、局部支持退化画幅）
  见 ARCHITECTURE §4.8。收口＝fitgrid/fitlayer/fitrange 三目录与 target 垃圾按用户 2026-08-28 裁定删除。下步＝B3 余量自由蒙版（已提交 `662b688`，见上条）。
- **🧭 共几何根因 + 氛围结构盲教义已提交 `10e02bb`（2026-08-27 晚，B1 复审返工 Codex F1–F8 + 共几何/教义主模型亲写 + B″ 实现 Codex
  gpt-5.6-sol xhigh 至活体步撞额度、主模型接管收口）**：
  **根因一**＝两侧分析缩略图独立取样，neutral.jpg 1600×1067→384×256 而 target.jpg 1600×1069→384×257，`structure_divergence`
  遇异长静默 `matched()` → `globally_same_content=true` → 结构证据门对 GUI/neutral 路径**一直关着**（RAW CLI 路径 257 vs 257 门开）
  ——步 4 起旗舰对 GUI 路径全部数字（0.0175/0.0180、步 8/9/10-B1 活体 A/B）皆门关态，「0.055 vs 0.018 的 3× 差」全由此来。
  修法＝`fit::analysis_pair`：源 `thumbnail(384,384)`，目标 `thumbnail_exact(源宽,源高)`（同一盒滤波算子；等形对按构造逐字节同），
  主路径 6 处 + 测试 12 处独立缩略图全部改走该助手；**根因二**（首版踩坑）＝目标用 Lanczos3 重采样、源盒滤波，核不对称单独把 p36
  同景对从 0.092→0.019/0.677 推到 0.107→0.034/0.537，改盒滤波后 p36/viaduct pin 转绿；死代码几何弃权分支删除。
  **门活体后的真机制**（旗舰对 D=0.49 氛围模式，秩配对 17 bin 存活：暗地 [0.12-0.29] 0.54-0.57 占帧 41%、地面自身被重绘的中间调
  [0.29-0.59] 0.10-0.33、天空 [0.59-0.82] 0.08-0.18）＝氛围估计器在存活范围上读出 −1 EV，但全局 EV 必移被扣留 59% →
  `moves_unsupported_luma_range` 复位 → 空配方 0.057→0.057，步 4 氛围模式按构造失效。三臂原型贴-测-还原（像素尺 ΔE76
  全帧/天空/地面，identity 23.5/37.0/12.4）：A 门赢 22.5/37.0/10.7、B 全豁免 12.0/20.3/5.3（饱和 +30 顶格）、B″ 结构盲证据
  12.9/21.5/6.0；门关基线 11.9/17.5/7.4。**用户裁定 B″ + 保持 ANALYZE_EDGE=384**（512 同瓦片 +4% 耗时、768 尺崩 +25-50%）。
  实现＝`EvidenceModel::structure_blind`（人口事实照样否决：单侧/空范围；结构存活与逐像素扣留关闭）；**每报告一把尺**（Full=
  结构模型；Atmosphere=盲模型，err/伤害/联合读数/置信/披露/区帧律全在其上，氛围解算自算盲尺 `err_before`）+ 氛围报告另携
  `FitReport.structural_evidence` 供 Full 分区（`attach_one_zone` 单一模式分支覆盖语义区/范围带/瓦片）与细节级（可辨识度是结构事实）；
  `rescore_report` 同尺（顺带携带 vouched-bands 注，为 p36 逐字节往返所需）；披露注 `FIT_NOTE_ATMOSPHERE_POPULATION_EVIDENCE`
  列出结构模型扣留的范围（en/zh）；主审把 `compose_report` 的 `expect` 改 `debug_assert`+`if let`（照片软件不因缺披露 panic）。
  测试：T1 结构盲保人口事实、T2 旗舰单尺、T3 p36 Full 往返 structural 缺席、T4 四旗舰测试重钉（地面区改名
  `calibration_land_zone_is_withheld_by_its_own_rerendered_mid_tones`：Full 区按自身成员扣留 [0.29-0.59] 且不含天空亮 bin）、
  T5 氛围帧内 Full 区读结构模型、T6 rescore 复现同尺；五条旧断言按新契约改写（饱和帽测试改钉顶格、扣留范围移动测试反钉、置信
  分离测试改钉同尺）。**活体**（新 exe vs 几何基线 exe）：neutral→target EV −1.00/7100K/+22.3/饱和 0/五点曲线，盲尺 0.189→0.096，
  置信 0.25；分割关附一瓦片 −0.56 EV、开附天空区 −0.08 EV；RAW 路径 EV −1.00 + 天空区 −0.27 EV（0.194→0.108）；像素尺
  10.6/16.4/5.9（关）12.5/20.6/5.9（开）优于门关基线 11.9/17.5/7.4；p36/viaduct 逐字节同；各两趟 SHA 同（仅蒙版存储路径随趟）。
  门 982(+9i)/15/145/2+2 双特性、clippy 0+0、i18n 0、字体 843、check_docs；集差 +6/−1 逐名；变异 Codex M1–M7 + 主审亲手
  MA 盲模型不清空间权重/MB 细节级读盲/MC Full 区读报告尺/MD 披露注省略 全红。Codex 用 Set-Content 把 8 文件写成 LF，收口时归一 CRLF。
  登记跟进＝CLI RAW 臂只拟合机内预览（main.rs:1230，解算域≠渲染域）、目标函数结构项在低可辨识度对上失活（core<100 px→matched）、
  ZONE_DROPPED 漂移 `{:+.3}` 披露把决定性漂移打成零、fitlayer/fitrange/fitgrid 三未跟踪目录归步 16；展示图重生成（步 15）触发再累积。
  材料：r30-materials/task-fit-atmos-blind.md、design-grid-as-analyzer.md §8–§9、task-b2-prequestions-report.md。
  下步＝B2 任务书 §3 按本批基线重写（上界在共几何、门开、B″ 尺下量；LOCAL_CEILING 不得挂 0.081/0.018）再派。
- **🔬 步10-B1 已提交 `49a796b`（2026-08-27，主模型亲写——Codex OAuth 额度见顶，按「实在不行就不用 codex」令）**：
  步10 改题「网格作局部场分析仪」（引擎网格判 DEAD：校准对网格 0.0043 但 4 亮度带投影 0.0030 已复现全部增益，
  报告 r30-materials/task-fit-bgrid-recon-report.md、设计 design-grid-as-analyzer.md，用户批准 B1→B2→B3）。
  B1=分区内证据视图：`EvidenceModel::scoped(tp, source_zone, target_zone)` 在**被移动的人口**上重聚合 17 亮度
  bin/8 色相带（目标 bin 按区内目标成员秩配对、按源:目标质量比；人口线 0.015、结构存活门 0.35、分歧折叠
  不变；全帧全一成员逐字节=原模型，具名测试钉）。三消费点=`attach_one_zone` 影调/色彩否决改问 scoped 视图、
  `spatial::read_tile` 瓦片权重/份额按自身几何视图（两侧份额同一人口）、否决覆盖=修正**移动**的栅格
  （`ZoneAttachment.coverage`：语义蒙版/亮度斜坡即自身，瓦片传栅格——估计器权重已带证据会把被扣留像素藏出
  人口）；盲动审计 5% 区域线改按 scoped 人口（`EvidenceModel::population`）而非整帧（深度 2 瓦片=整帧
  6.25%，整帧线下半瓦扣留永不成「区域」=步9 潜在缺口）；色彩被扣留时跳过线改问仅亮度残差（与随后验收同一
  量）。根因=整帧证据株连分区（校准对范围层找到带 [0.118,0.294] 却被 `luma[0.18-0.41]` 零证据否决——被替换
  天空占同 bin）。旗舰对语义路径实证：地面影调否决消失（vetoed=false），仅亮度残差 0.0043<0.012 跳过线→
  「已匹配」，全残差 0.0456 为色度（Blue 单侧扣留），帧保持天空-only 0.01795（首版放行 +0.10 EV 换
  0.0043→0.0023 反把帧 0.01786→0.01927，主审拦下补跳过线）。合成夹具 384²（三分之二帧扣留、帧 bin6 有
  人口零权重、地面视图保 bin5/6、天空视图扣 bin6）+ 瓦片覆盖否决夹具（上半被替换、下半 +0.12EV：估计器
  权重下不可见、栅格覆盖下影调扣留、瓦片不附着）。调试电池首跑揭两处与 B1 无关的 debug-only 红：范围
  refit 测试违反 `fit_recipe_from_promoted_with_disclosure` 校准-only 基线契约（debug_assert，此前门全
  `--release` 从未编译）→ 测试改从契约线下进入 `attach_luminance_ranges`；三条空间夹具测试经
  `source_weights` 注入证据（read_tile 不再读的缝）→ 统一 `pretend_full_support` 从成分注入。
  活体 A/B（步9 HEAD exe vs B1，各两趟 SHA 全同）：GUI 路径（neutral 显影）旧=天空 −0.08EV+地面 +0.08EV（地面自身残差 0.041→0.045 恶化）+瓦片 r2c0（−0.24EV、增益 1.30/0.86/0.79）帧 0.0175 → B1 仅天空 0.0180（地面仅亮度残差 0.002 判已匹配；r2c0 随后过不了零漂移帧律——scoped 瓦片权重与帧律整帧权重两种货币，B2 正视）；RAW CLI 路径旧 0.0549→0.0427→0.0345 → B1 0.0549→0.0452→0.0369（带 [0.118,0.294] 仍扣留=值域带自身人口就是盲的，B1 前提对值域带证伪，网格赢在位置×亮度只有瓦片/B3 能吃；r2c0 暖增益被区内单侧 Blue/Purple 否决，旧靠整帧份额漏放）；耗时 73→110 s/149→168 s（三块临界瓦片入围后被附着份额门拒：冻结原始份额 vs 稳健合成份额两把尺待统一）。**用户裁定「按规矩来，接受数字略差」**。 env 门 HEAD 等价探针（工件=步9 HEAD neutral）语义半边按设计分歧（理由 3102B vs 2788B：已匹配注替代地面附着）。 门 971(+9i)/15/145/2+2 双特性（集差按名 +6/−0：a_zone_is_judged_by_its_own_members_not_the_frames_bins、a_ground_zone_is_not_vetoed_by_the_sky_it_does_not_touch、calibration_land_zone_is_no_longer_withheld_by_the_replaced_sky、a_zone_whose_movable_class_already_matches_is_left_alone、a_tile_reading_keeps_the_mid_tones_the_frame_withheld、a_tile_is_vetoed_over_the_raster_it_moves_not_its_estimator_weights）、clippy 0+0（SupportField 收 8 参）、i18n 0、字体 843/843、check_docs 23P/--gates 25P 0F（README/ARCHITECTURE 计数 974→980 同步）；release 电池首轮 pipeline/store 各偶发 1 条 fs 类红（pipeline.rs:3908 write_xmp unwrap、store.rs:8356 backup_saved_develop unwrap；单跑与终电池全过，登记负载下抖动待查）；变异 8 条亲跑全红（A scoped 忽略分区/B 否决读整帧/C 瓦片沿用整帧权重/D 存活门关/E 跳过线删/F 目标秩不分区/G 覆盖忽略/H 区域线回整帧；B 首跑串陈旧=假绿，修串后单跑红——变异串须随改行同步）。登记跟进=中性拨盘区无注静默丢弃、窄亮度范围斜坡区配对影调解
  无支撑结无解、色相 scoped 仅靠全帧同一性钉（无独立变异）、env 门 HEAD 等价工件为步9 HEAD（语义半边按
  设计分歧）；下步 B2=`src/fit_field.rs` 局部场分析仪（固定 λ=1/s=1）+ 形状门/LOCAL_CEILING 披露+停机。
- **🧩 步9 已提交 `67084b2`（2026-08-27，Codex 三会话铺至编译绿后接连 OOM/1450/额度 403，按用户裁定主模型接管亲审亲收口）**：
  分层空间反推=全局 →（语义 或 亮度范围，互斥）→ 冻结证据四叉树瓦片（`src/fit_zoned/spatial.rs`）：
  4x4 叶层、附着上限 4、每次附着后重渲重推导；门=双侧冻结份额 ≥3%、原始 D<0.65、加权 95% CI
  排零、|子−父|≥2/255、共享估计器、边界 rim ≤0.012（保方向二分收缩）、**零帧回归容差**（经
  `ZoneAttachment.frame_regression_tol` 缝，语义区仍 0.02）；瓦片=现有 `Bitmap`+`Custom`（schema
  纪元 1 不变），经典 XMP 具名位图损失跳过、配方 JSON 无损；栅格所有权 `OwnedRaster::claim_sibling`
  （拒绝只删自己、接受耗路径）。受门引导精修 `src/mask_refine.rs`（零依赖、半径 8、ε=(4/255)²、
  2r 领口外逐字节还原、覆盖漂移 ≤0.002、Sobel 引导边对齐不得降；仅语义剪影与瓦片领口，**绝不亮度
  范围**——生产调用计数钉死）。监督验证揪两缺陷根治：①每代全图遍历转录把持久化理由串顶爆 4096
  上限、尾截吃掉两瓦附着披露（配方有瓦、理由无字）→ 每代聚合成一条具名扫描注（逐节点 id+原因；
  叶候选保全读数）+ `MAX_RATIONALE` 提 16 KiB（旧上限在预批管线上就已截 12B）；②HEAD 等价工件
  误用 source.arw 且旧二进制自截理由串 → 工件改 neutral.jpg、断言改「头理由串=当前串钳位前缀+
  余字段逐字节等」。活体（旗舰对，双生产者，各两趟 SHA 全同 `6df1baf3…`/`89ca2532…`）：r2c0→r3c0
  两瓦（冻结份额 3.6%/3.8%、D 0.303/0.438、CI ±0.0006），合成帧 0.0549→0.0427→0.0345 单调改善，
  变更天空全体源份额弃权=冻结证据法则实证；语义精修两侧正确弃权（对齐 0.011→0.009 / 0.017→0.014）、
  瓦片领口精修保留；HEAD 等价 1 passed。门 965(+9i)/15/145/2+2 双特性（集差 +16/−0 按名）、clippy
  0+0、i18n 0、字体 843/843（新增 代/父，五子集自捐赠重建——缓存捐赠两件为截断下载，自 google/fonts
  重取）、check_docs 平 23P / --gates 25P 0F；十变异亲跑全红（M-9-G 首绿=测试太弱：python 复刻实证
  窄领口截断补偿尾巴使覆盖漂移 0.00012→0.0041 爆门，固定装置并入具名测试后红）。报告归档
  r30-materials；登记跟进=本节 15 条超「近五条」约定待归档、fitlayer/fitrange 未跟踪待收口清理。
- **🧵 步7b 对应场接线估计器：D 门单源自动咨询 + Full 区置信加权/流场重映射（2026-08-26，主模型亲写，用户裁定「完全自动」）**：
  `fit::CorrespondenceProvider` 闭包缝贯通全部入口（`fit_recipe_with`/`fit_recipe_from_with`/
  `fit_zoned::fit_recipe_zoned_with`，旧签名零变化=测试面天然隔离）；**咨询点唯一**在氛围
  支路（D≥`DIVERGENCE_GLOBAL` 才咨询，Full 对绝不为对应买单——变异 M-7b-B 钉死；模式选择
  永不读场）；`correspondence_for_pair` 把 48² 场投影到本对栅格（格内偏移随流刚性搬运，
  恒等场+等几何=逐字节无操作——守恒律一半；几何不等时归一化重映射顺带修正行剪切，语料
  target 1600×1069 vs 源 1067 实测 land EV ~0.015 差即此效应，文档现场论证）；FitReport
  携带 `PairCorrespondence`（进程内如 evidence），逐区消费：**只有 Full 区**组合置信×稳健
  权重并对 `c.tp`（对应位置读取的目标）配对/求矩/refit/t_cdf，Atmosphere 区与所有份额门
  保持前场语义（分歧永不丢区的守护——份额门永不读置信、弃权场整体回退，变异 M-7b-D/E 钉
  死）；帧漂移保险 `zoned_err` 保留原始 tgt_px（诚实分界）。CLI match 与 GUI 反推 worker
  同一 provider（`correspond::fit_provider`：1024px 暂存双帧→侧车→清理，失败降级进披露注
  `FIT_CORRESPONDENCE_UNAVAILABLE({e})`）；两把新钥匙+中文对（字体重子集 842/842，况/歧/跨
  三字形补齐）。**实测**：zoned A/B 语料对（场开 vs 场不可用）——拨盘逐字节全同——该对 land 区修正两侧均被证据门（899798e）先行扣留，场的作用点在其下游，仅披露注不同（场开=实测 59%/0.80，场禁=不可用注）；即旗舰对零回归，复跑噪声地板=零（连 DIFT 场都逐字节复现，仅蒙版声明文件名后缀异）；场的增益由估计器级平移恢复测试钉住（24px 平移打断同下标配对→重映射恢复真调子映射 err<0.03）；披露注实测
  「59% confident counterpart, median 0.80」与 7a 仪器读数一致。**风险表重裁**：四会翻测试
  （content_divergent_calibration/invented_sky_gradient/calibration_sky_zone/四 permutation
  预算）全数保绿——保守接线（模式不动、氛围臂不动、测试不传 provider 即旧路径）使其免翻，
  新增守恒律测试钉住带场行为（恒等场不改任何 dial、零置信场整体弃权、24px 平移场恢复被平移
  打断的配对 err<0.03）。**门**：clippy 双特性 0+0；电池 942(933+9i)/15/145/2+2（+5 具名：
  remap 恒等/平移恢复/门单源/恒等披露不改 dial/zoned 守恒）；五变异亲手红（M-7b-A 格内偏移
  /B 门失守/C 重映射读源格/D 弃权删除/E 场臂丢稳健）。**登记**：台账 7a 条所记「三调用点」
  实为两活点——全局 Full 站点在 D 门下不可达对应场（Full⟹D<0.35⟹无场），未接死代码，
  现场注释与本条如实修正。

- **🔗 跨图对应仪器：DIFT/SD 2.1 第四侧车 + 共享执行器上提（2026-08-26，主模型亲写，计划步 7a）**：
  用户裁定骨干=DIFT（SD 2.1 扩散特征）。`python/correspond.py` 第四侧车：复用
  `denoise._fetch_verified`（下载-校验单实现），11 文件 sha256+字节帽全钉在
  `sd2-community/stable-diffusion-2-1@bb2154823665…`（官方 stabilityai repo 已下架——
  匿名 401/带权 404，2026-08-26 验证；镜像三塔 fp32 摘要与独立上传者 SfinOe 逐字节同一
  后才钉定；RAIL++-M 允许商用，限制为行为条款非领域条款，与 SegFormer 落线不同侧，
  ARCHITECTURE 家族表已如实记载）；`local_files_only` 四处装载、确定性旋钮全套、
  tmp+fsync+replace 发布。DIFT 配方=论文设定（t=261、768² 双线性、`up_blocks[1]`
  1280ch@48×48、8 抽签逐一跑限 VRAM）；匹配=互近邻，**置信=循环一致×3×3 中位流平滑
  （σ=1.5/2.0 格），raw cosine 仅诊断导出不进置信——置换夹具克星按设计进场**（7b 防
  氛围夹具翻转的前置）。`src/correspond.rs` 桥：解析门（网格 48² 拒偏、四数组等长、
  坐标域 [0,grid)、置信 [0,1]、余弦 ±1.001、有限性），成功产物**保留**（诊断口交付物，
  与 embed 删中间件相反、现场论证），坏场丢弃不留伪结果。**rule 09 上提**（复制守卫
  四连拦、全部按「先削源后立」落地）：`lib.rs::run_model_sidecar`（embed↔correspond
  同形运行序列单实现，embed.rs 净减 54 行改调用）、`lib.rs::with_model_slot` 升进程级
  单飞（SigLIP 0.75GB 与 SD2.1 2.4GB 永不同驻，embed 观察测试转钉 crate 级门）、
  `write_stand_in`/`test_dir` 测试基建收编（denoise 桩×2+内联夹具×2、claude tdir、
  pipeline 内联×1；GUI tests.rs 三份跨 crate 够不到 lib 的 cfg(test)，合法保留）。
  CLI `correspond <source> <target> [-o]` 诊断口：L09#1 预检序（坏 -o 在解码与 2.6GB
  首跑下载前拒收）、经 `decode::preview_only` 暂存双帧、汇总打印（中位置信/覆盖率/
  平均流）。配置：`correspond_script` 字段 + `AUTOSHOP_CORRESPOND_SCRIPT`
  （Destination、env-only、信任测试登记）+ 九处全字面量构造点同步。**实测（权重
  2.6GB 首跑经摘要门全过）**：身份对 median 1.000/覆盖 100.0%/流 0.00 格（零点精确）；
  分歧语料对（生成天空）整体 0.801/59.7%/3.66 格，**天空格 median 0.009（覆盖 21.5%）
  vs 地面 median 1.000（覆盖 90.5%）——仪器精确分离被替换与被保留内容**。**门**：
  clippy 双特性 0+0；电池 937(928+9i)/15/145/2+2，集合差 +13 库（10 解析/argv/源不变
  量 + 3 loopback 含共享执行器 exit-0 守卫）+1 CLI 具名；五变异亲手红
  （平滑度项失守/修订钉移动/坐标门失守/执行器删 sidecar_wrote/字节帽掉——家族登记
  生效性）。**登记跟进**：denoise/segment 各自运行序列变体（暂存转档/stdout 报告/
  探针口）收编 `run_model_sidecar`、claude.rs stand_in 收编 `write_stand_in`。
  **下一批 7b**：按 D 门把对应场接进估计器（pair_weight 闭包+tp 重映射三调用点），
  风险表四会翻测试逐一重裁。

- **🖼 整图生成贴合原图：提示词硬化 + 生成侧 D 披露 + 自选有界重试（2026-08-26，主模型亲写，计划步 6）**：
  `reimagine` 在 `fidelity=high`（CLI 默认、GUI 固定）把提示词组装到**无条件保真前导**上
  （要求模型「重新显影同一张照片」，禁增删移改内容），用户 Direction 附于其后而非替换——
  修复两处既有缺口：① `input_fidelity` 请求参数在 gpt-image-2 上被能力协商静默丢弃，
  主力模型上线上原本**零**贴合信号；② GUI 用户一旦输入 Direction 就替换掉唯一的保真
  句（旧空栏回退句）。`low` 维持文档化自由发挥（原文直达）。**生成侧贴合度量**：生成后
  用反推自己的统计（`fit::structure_divergence_for`，同函数同栅格同中性基）测送入帧 vs
  生成帧的结构差异 D 并双面披露（CLI println + GUI 落地注，D≥0.35=`DIVERGENCE_GLOBAL`
  即预警反推将走氛围模式；阈值 pub 化单一定义）。**自选重试**：`--fidelity-retry` /
  GUI 复选框（Prefs 五点惯例，双默认 false=付费永不默认开；进 Config 会让 cwd 配置
  文件可拨计费行为故不进）；D 超阈时限一次再买一张，保留 D 较低者，弃用侧 D 一并披露；
  与「2xx 后禁重发」规则的区别（这是显式二次购买而非同单重发）在现场注释论证。实测
  夹具：回声帧 D=0.000、噪声帧 D=1.892（阈 0.35 前提牢固）。**门**：clippy 双特性 0+0；
  电池 924(915+9i)/14/145/2+2，集合差 +4/−0 具名
  （`the_faithfulness_scaffold_is_unconditional_under_high_fidelity`、
  `a_reimagine_hardens_the_prompt_and_measures_its_result`、
  `the_divergence_retry_is_opt_in_bounded_and_keeps_the_closer_result`、
  GUI `the_reimagine_fidelity_retry_is_off_in_both_defaults`）；audit_i18n 八项零、
  字体重子集 839/839（付花钱阈偏弃等新字形）。**回滚面**：`hardened_prompt` 恢复透传 +
  measurement/retry 块删除 + GUI 五点撤销，`ReimagineReport` 是库 API 变更（v1.1 发版
  义务清单需记）。默认零新增 API 开销（度量本地、重试 opt-in）。

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
- **R30 Step 8 — 自动亮度范围分区（本批）**：保留「全局 fit 优先」，并把
  本地生产者定为互斥：天空分割成功时沿用语义天空/地面位图路径且不推导范围；
  分割被禁用或不可用时，纯 Rust 路径在既有 17-bin 证据上按 `0.03` 有符号
  残差组成连续段，最多四段，按当前渲染从暗到亮各拟合一次。相邻 ramp 为
  `1/17..=2/17`，估计权重总和归一到 `≤1`，整栈最终共用 `0.012` value-rim
  门与保方向二分收缩。每段沿用 `attach_one_zone` 的稳健配对、证据、占比、
  correspondence、局部质量与参数化帧门；语义区保留 `0.02` 漂移保险，亮度范围
  使用 `RANGE_FRAME_REGRESSION_TOL = 0.0`，合成证据加权帧变差即逐段放弃；
  零可接受段保持全局配方逐字节不变。
  持久化使用 `MaskRole::Custom`、确定性英文名与全画面 LINEAR 哨兵交集，故
  recipe schema/XMP grammar 均不升级；本批不产出 color range。合并、逐段
  放弃、收缩 k 与零差分仍失败均走 typed rationale，GUI 卡显示原生亮度范围
  及四个有序边界。
  **登记观察项（主审裁定）**：Full 模式范围带的色彩增益无独立帽——带自身
  D<0.65 走 Full 与语义区同 D 同规则并非不对称，且修正后的秩配对派生在旗舰
  对上不再提出大色彩方案（修正前位置配对曾提出 [1.30,0.87,0.75]、被帧门弃）；
  若未来出现「过帧门却发明色彩」的实例，跟进带级分歧比例帽。
- **R30 Step 9 — layered spatial fit and gated mask refinement**: shipped order
  is `global -> (semantic OR luminance ranges) -> quadtree tiles`; the first two
  local producers remain exclusive. Spatial derivation intersects normalized
  rectangles with evidence frozen from the original pair, traverses best-first,
  stops at depth 2 (4x4) and four accepted leaves, and re-derives after every
  attachment. Both evidence shares must be at least 3%, original `D < 0.65`, the
  weighted 95% interval must exclude zero, and child/parent residuals must differ
  by at least `2/255`. Tiles share the robust estimator and `0.012` rim gate but
  use zero composed-frame regression tolerance. They persist as existing Custom
  bitmap adjustments at a 2048 long-edge cap: recipe JSON is lossless and
  classic XMP emits the named bitmap loss, with no gradient approximation.
  Dependency-free guided refinement (radius 8, epsilon `(4/255)^2`) runs only
  before semantic/tile fitting, restores every non-collar pixel, caps coverage
  drift at `0.002`, and abstains when Sobel guide-edge alignment decreases; it
  never touches luminance ranges. No recipe-era change, no new toggle, and
  multi-class semantic production remains out of scope.
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

## v1.1 发版义务清单（进行中）

- **反推逐带 HSL（步14 收官批 `ab01520`）——硬渲染变更**：release notes 必须说明 v1.1 起反推在默认强度就会写
  `hsl.saturation`/`hsl.luminance`（`hsl.hue` 恒 0），同一 (源,目标) 对产出的配方与渲染不同于 v1.0.x；持久化
  schema 不变（`hsl` 为既有字段，recipe.json/XMP 往返不受影响），旧配方读入行为逐字节同。实测：p36 成品误差
  0.032592→0.031792（置信 0.6657→0.6752）；校准对经既有 1e-4 容差发出 Red/Orange +9 sat/+9 lum（用户裁定保持容差）；
  石桥 0.030419。四条摘要串收窄为「local masks and per-band hue rotation are not recovered」+ unrepresented 解空间
  列表加该阶段=持久化 rationale 文本变化。NumPy 场天花板钉按文档三步重推 0.0700225→0.0677020。
- **风格索引 v5 蒙版习惯 10 宽（步14 收官批 `f03b08a`）——前向不兼容**：新构建写出的索引 `masks.*.mean` 为 10 宽
  （加蒙版内 temperature/tint）+ `curved` 字段；旧构建读新索引报 `invalid length 10` 须重建；新构建仍读 8 宽 S3 形
  （缺列补零）故版本门的可操作报错可达。`--reference-image`/`AUTOSHOP_SEND_REFERENCE_IMAGE`（Destination 信任）为
  新可选开关，默认关=行为不变。
- **检索权重重标定（步14 收官批 `13c262e`）**：release notes 必须说明 W_TXT 4→0.5 且文本项改为先减去每候选
  hubness 再 z-score（MAE 0.688864、CI [+0.005837,+0.041111]；反义 top-1 71%→44.7%、检出 52→149/169）；
  `AUTOSHOP_STYLE_TEXT_WEIGHT` 语义随之更新；零权重路 bit-for-bit v5。
- **GUI OAuth 图像模式修复（步14 收官批 `a6e5a03`）**：release notes 必须说明已发布的 OAuth（Codex bridge）图像开关
  下 Reimagine 此前根本跑不完（桥把 JSON 正文标成 SSE）+ 订阅档封顶尺寸被拒；v1.1 起嗅探正文、承认同纵横比封顶
  （长边≥1024、容差 0.5%）并在终端/GUI 双面披露实收尺寸。

- **空间瓦片的边界连续性门（v1.0.x 确诊空转，v1.1 已根修 `0ecc2e0`——硬族改跨边界台阶差分中的差分、软族保原 rim 尺（typed 双尺，实测两尺 3/10 翻判不可互换），零可测过渡=拒绝非通过；石桥接缝实测见台账）**。原确诊记录如下，保留为证据：
  `boundary_line_rims`（`src/fit_zoned.rs:421`）只从蒙版权重落在
  `[ZONE_BOUNDARY_LOW=0.05, ZONE_BOUNDARY_HIGH=0.95)` 的像素取读数；而空间瓦片的
  `attachment.source_weights` 是被**硬矩形谓词** `in_tile` 框出来的冻结证据
  （`src/fit_zoned/spatial.rs:138-142`，`:207` 直接写 255/0），根本没有过渡带 →
  `boundary_rim` 返回 `BoundaryReading { rim: 0.0, transitions: 0 }`，
  `enforce_bitmap_boundary` 的 `initial.rim <= ZONE_BOUNDARY_RIM_MAX` **空过**。
  实证（2026-08-30 石桥反推）：四张瓦片的 rationale 都是
  `Spatial tile d2r0c3 passed the boundary gate: signed rim 0.000 -> 0.000 …
  (budget 0.012, 0 measured transitions)`，而**同一帧**的语义区门读到
  `candidate rim 0.312 … 492 measured transitions` 并正确拒绝；渲染出来右上角天空
  留下肉眼可见的矩形接缝。既有测试 `tile_boundary_shrink_preserves_direction_and_budget`
  （`spatial.rs:1209-1246`）同因空过——它的 `boundary_fixture` 也是 0/255 硬蒙版。
  **这道门从未拦过任何东西。** 2026-08-31 用户令「有肉眼可见的影响，就加上根修步骤」——
  **根修步已排（紧接当前、收口清账之前）**：跨边界台阶（差分中的差分）一把尺量软/硬两族蒙版，
  验收含石桥接缝以 `scripts/rim_overshoot.py`（无蒙版尺）实测消失或该瓦被拒；任务书
  `r30-materials/task-fit-tile-boundary.md`。修成后本条从「如实披露」改「已修」。

- **风格检索扩容 S1+S2（步14，合并 `74a1e93`）**：release notes 必须说明 v1.1 起风格参考多了三把可选的相似度尺——
  SigLIP 2 图像向量（`--embed` / GUI「Use image embeddings」偏好，默认关）、Direction 文本对候选图像（W_TXT=4）与
  本地描述互比（W_DESC=0.5，`--describe` 需同时 `--embed`，Qwen3-VL-2B 本地侧车，首次下载 4.3 GB 权重、CUDA bf16 约 4 GiB 显存）；
  开关全关时检索/参考块/配方对 v1.0.x **逐字节同**（具名测试 `default_style_reference_is_byte_identical_to_head`），
  唯一默认可见变化＝建库进度改按四阶段汇报（帧→图像→描述→文本）。成片外观库（`looks`，≤500 张 JPEG）只进提示词与参考图，
  永不进 `style_targets`/`blend_toward`；Style≥0.85 参考块「TARGET」措辞不变。索引文件新增可选字段（`embed`/`tags`/`desc`/
  `desc_embed`/`looks`），旧索引照常读、新索引被旧构建读时按具名测试行为。描述缓存 `style-descriptions.json` 落在用户存储根，
  键＝帧字节 SHA-256+模型+提示词版本，20,000 条上限。发版链：CI gui 测试步改为 `--bin autoshop-gui`（gui 特性只加依赖），
  `check_docs --gates` 按套件名取数。权重常量由 `the_shipped_text_variant_is_the_measured_one` 钉住，重标定转录随批归档。
- **线性渐变落差剖面硬变更（`817fa13`）**：release notes 必须说明 v1.1 起线性渐变
  蒙版的覆盖度剖面由夹紧线性斜坡改为 C¹ Hermite smoothstep（3t²−2t³），两手柄
  处一阶导为零；含线性蒙版的旧配方重渲像素会变（手柄间过渡变软，手柄与两侧
  平台逐字节不变），径向/位图蒙版逐字节同 v1.0.0（各自具名测试钉住）；XMP
  schema、手柄输运、MaskFrame 法则零变化。依据：Lightroom 手工渐变探针两端转折
  80/80 行 vs Clamped 0/3，`scripts/linear_falloff_probe.py --fit` 自由端点拟合
  smoothstep rms 0.0045 vs 线性 0.0169（用户裁定 2026-08-28）。可粘贴的中英段落
  在 `target/linear-falloff/flip-report.md`「Ready-to-paste v1.1 release note」。
- **强度轴支配反推预算（步11 `302efb1`）**：release notes 必须说明 v1.1 起 `match --strength` /
  GUI 面板 Strength 决定反推诚实预算：0.65 默认逐字节同 v1.0.x（超预算 WB 仍 as-shot），
  唯一默认行为变化＝方向一致的全局色偏在每档强度都被测量（置信/拨盘可变，rationale 有
  `FIT_NOTE_GLOBAL_CAST`）；默认以上 WB 沿流形收缩并 typed 披露，≥0.85 否决改为披露
  + 置信封顶（0.414@0.85、0.35@1.0）。Style≥0.85 参考块改「TARGET style」措辞、
  `style_pull` 取代 0.6 帽（0.3 出厂值 0.18 不变）；strength>0.70 且 Style<0.85 不再收到旧
  committed 层 FLOOR 措辞。深层路径的 rescoring 现在重推导高强度披露并保留封顶。
- **多区域语义分区（步12 `a2173c9`，合并 `32b0fe4`）**：release notes 必须说明 v1.1 起 `match --zoned --regions 2..4`
  与 GUI「Up to four semantic regions」复选框（Prefs 新键 `zoned_four_regions`，旧 prefs 经
  `#[serde(default)]` 解为 false）开启最多四个 disjoint ADE20K 类区域（每区独立选 Full/Atmosphere，
  置信取最差已接受区域）；默认仍是历史天空/地面双区路由，拨盘+置信对 v1.0.x 逐字节同，唯一
  默认行为变化＝共享 `attach_one_zone` 对「已匹配不需修正」的区域新增 typed `ZONE_ALREADY_MATCHED`
  rationale 句（仅 rationale）。四区试跑与种子化双区结果在**同一把证据尺**上仲裁，不优于（含平局）
  即整体退回双区结果并附 `REGION_FRAME_REFUSED{multi,two,regions}`；多类层不可用 / 无区域过支持
  下限时 typed 交接 `SEMANTIC_REGIONS_UNAVAILABLE{e}` / `SEMANTIC_REGIONS_NONE{n}`；区域边界门拒绝以
  `REGION_BOUNDARY_REFUSED{label,why,before,max,transitions}` 如实披露（不伪造 after/k）。每次多类
  fit 恰两次 OneFormer 推理（源/目标各一，双区种子化不再推理）；manifest 64 KiB 上限、平面路径单一
  文件名且拒绝链接；位图仍引擎专属，recipe schema / XMP 零变化。
- **自动范围行为**：release notes 必须说明 zoned 入口始终先做全局 fit；
  语义分割成功时保持既有天空/地面位图结果，禁用或不可用时才自动尝试亮度
  范围，无可接受段时保留全局结果。本批没有新 CLI/GUI 范围开关，也不产出
  color range。
- **哨兵宿主投影**：说明原生亮度范围以 observed-domain 全画面 LINEAR
  哨兵承载并作为 intersect range 写入 Lightroom XMP；位图语义分区仍只由
  本机引擎渲染。`MaskRole::Custom` 与既有 `LocalAdjustment.range` 意味着
  recipe schema era 不变。
- **逐段拒绝披露**：release notes 与 GUI 必须保留每个亮度区间的 attach /
  abstain / merge、边界 rim、共享收缩 `k` 及 typed refusal；不得把单侧或零
  结构证据静默解释为「相等」。
- **Automatic layered order**: release notes must state `global -> (semantic OR
  luminance ranges) -> spatial tiles`, frozen original-pair evidence, depth-2
  and four-tile caps, re-derivation after attachment, and zero tile frame
  tolerance. Semantic/range exclusivity remains deliberate.
- **Engine-only spatial projection**: tile bitmaps remain lossless in recipe
  JSON and are omitted from classic XMP with the existing named bitmap loss;
  no four-gradient approximation is implied.
- **Conservative refinement**: release notes and GUI facts must distinguish
  kept from abstained semantic/tile refinement, state that the normal rim/frame
  gates rerun after refinement, and state that luminance ranges are never
  refined. Multi-class semantic masks remain explicitly out of scope.

### v1.1 收口裁定（2026-08-30，主审记录在案）

- **色彩范围区**：判 v1.2。逐带 HSL（4a）已覆盖「按色带选人口改色彩」的主面；独立 color-range 蒙版生产者是新特性非缺陷，发版窗口不再开渲染面。
- **CE 冗余清理批 + 超预算文件拆分**（store.rs 8906 / check_docs.py 955 / i18n.rs / ARCHITECTURE.md / calibrate_style_retrieval.py 764）：判 v1.2。纯结构搬迁，收口窗口动它回归风险大于收益。
- **4a′ 合成 Full 钉 + `UNREPRESENTED_HUE_DEG` 带质心路测试**（fit-hsl 登记）：判 v1.2 测试补强批。
- **逐方向 β**（OLS 斜率 0.247–2.000）：不拟合——12 个文本上的自由参数，登记保持。
- **`style-query` 未打印 `txt_hub_corrected` 披露位**：判 v1.2 小批。
- **描述缓存跨库 GC**、**侧车家族样板重构**、**AdherenceTier 命名**、**staged frames 累积**、**四重名 stem**：判 v1.2。
- **`W_LOOK` 归一化不可测**：登记保持（谐波在校准尺外，无法上尺）。
- **xmp 普查钉值刷新**：本收口已完成（见台账条）。
