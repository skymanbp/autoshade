# ROADMAP — “一定程度直接取代 Photoshop” 路线（v0.5.0 之后 · UX 阶段）

> 交接文档：每项都附实现要点与 `file:line` 锚点，供新会话不重读全库即可
> 开工。更新于 2026-07-24（**v0.12.0 已发布**：全量 debug + 协同性审计
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

## 第十轮计划（🔒 用户锁定 2026-08-09 —— ✅ 已完成，v0.23.0；执行记录见「当前状态」首条）

> 用户原话（2026-08-09）："GUI更进一步打磨……完成后，进行一次cct插件的代码攻击式
> 审阅，你负责统筹规划和监督，任务交给gpt 5.6 sol最高推理等级来执行。攻击面要广，
> 深度要深，每个node都要到收敛。确保bug free。同时你自己进行一次结构式代码审阅，
> 进行并行检查。"

**Phase 1 — GUI 打磨五项（全部必做）**

1. **美观现代化**：现观感"僵硬"——整体视觉风格打磨（间距节奏、圆角、层次/分组、
   克制的悬停与过渡反馈），向现代桌面调色应用看齐。
2. **文字与排版**：当前"看着有点晕"——字号层级、行距、面板分组降噪、对齐网格；
   字体与颜色体系一并打磨。
3. **双主题**：亮色 + 暗色两套完整方案（egui Visuals 双套定义，随 Prefs 持久化，
   切换即时生效；两套都要过对比度检查）。
4. **按钮打磨 + "□" 豆腐块根治**：根因已核实——GUI 仅有 egui 默认字体 + 运行时
   系统 CJK 回退（`install_cjk_font`，gui.rs），👁 ⧉ ⊕ ⊖ ⇱ ◌ ▭ ◯ ∩ ◭ 等符号
   两边都无字形 → 渲染为 □。修法方向=嵌入符号覆盖字体（或精选图标字形子集）
   进回退链 + 全 GUI 符号清单逐一核验渲染；按钮样式（尺寸/留白/主次）同步打磨。
5. **多语言实现检查与对齐**：en/zh 全量 key 对齐审计（含第九轮新增 40+ 串），
   缺译/漂移/过时措辞清理；新 UI 文案两语言同步落地。

**Phase 2 — 双轨审阅收口（Phase 1 完成后）**

- **cc-tree code-audit 攻击式审阅**：本人（Claude）统筹规划 + 监督收敛；执行引擎
  =Codex **GPT 5.6 sol，最高推理等级**（用户指定；若该模型 id 在 codex CLI 不可用
  →如实上报用户，绝不静默换模型）。要求：攻击面要广（安全/性能/正确性/契约全维度）、
  深度要深、**每个 node 按 cc-tree 收敛判据跑到收敛**，目标 bug-free。
- **结构式代码审阅（并行）**：本人独立执行（架构/职责/契约/状态机/跨面一致性），
  与攻击审阅结果交叉比对，发现全修 + 回归钉死。
- 收口照旧：clippy/全测试/文档五份+记忆同步/发版（预定 v0.23.0）。

## 第十二轮计划（🔒 用户锁定 2026-08-09 —— ✅ 已完成，v0.23.2；执行记录见「当前状态」前三条）

> 用户原话（2026-08-09）："0.23.2更新路线图：1.完成先前所有搁置/未收口的
> 工作，不要留pending。2.UI图形界面继续美化。3.完善'保存'功能，让导出图片
> 正规化，比如'导出到...路径'、'选择保存照片格式'等等。4.操作手感优化。
> 5.检查屎山代码，确保代码条理清晰，保证可读性和可维护性。可以使用code
> graph和fuck u code，也可以检查函数/代码loc或者函数复杂度。" 追加：
> "gui方面，一定要注意'高级感'、'现代感'。"
> 结构化拍板（AskUserQuestion，2026-08-09）：①搁置范围=**15 簇全清，含
> 翻案 C1/F10**（撤销"仅登记"前拍板，本轮实修）；②**GUI 启动本轮解禁**
> （截图自验+真机 E2E；轮末禁令自动恢复）；③审阅=**16 路 codex exec
> gpt-5.6-sol 最高推理（xhigh）并行全扫 + Codex 复审**，Claude 负责监督/
> 核查/统筹规划；④导出收**对话框（路径+文件名）+ 格式与参数**两件
> （尺寸控制、批量导出未选=不做）。

**阶段 0 — 屎山测绘（轨道⑤前置，只读）**：LOC/函数复杂度地图（wc/tokei +
clippy `cognitive_complexity` 临时门 + `cargo modules` 依赖图；fuck-u-code
如可用且支持 Rust 则并用，否则如实上报换等价工具）→ 产出热点清单（已知
最大巨石：gui.rs 15578 行）与 gui.rs 分模块边界设计。

**阶段 1 — 结构手术（行为零变）**：在册"结构"簇——gui.rs 巨石分模块
（1781 处 self 引用）、Opened 恢复块（历轮缺陷热点）抽纯函数、
read_saved_develop 4 元组 / save-all 6 元组结构体化。门=全测试逐项等集
（set-diff，非总数）+ clippy 0。

**阶段 2 — 架构四大簇**：披露穿线（统一"警告聚合→UI 通道"，write_xmp
损失注记 7 调用方全消费，L08 全 10 + L15#2）；内存边界统一（L02 全 13 +
L07#6/#7 + L16#2/#4）；并发/TOCTOU（L01 全 6 + L10#2/#6）；耐久性
（L03 全 6）。细目见下方在册清单。

**阶段 3 — 中小簇清零 + C1/F10 翻案**：XMP 保真 4、跨面一致 4、GUI 状态机
5、sidecar 加固 5、测试质量 4、i18n 4、渲染数值 3、CLI 3、Unix setenv 1
（细目见在册清单）；C1/F10 词法路径身份翻案实修——store 路径键从词法拼写
升级为规范化身份（canonicalize 失败回退词法+披露；既有词法键做迁移/双查
找），落地前按原始 C1/F10 记录重审再定案。

**阶段 4 — 导出正规化（用户项③）**：GUI「导出到…」另存对话框（rfd
save_file：路径+文件名+记忆上次导出目录）+ 格式与参数（JPEG 质量滑杆 /
PNG 8/16 位 / TIFF 8/16 位；色彩空间沿用 EXPORT_SPACES 三选）；save_jpeg
布尔升级为结构化导出设置并随 Prefs 持久化；en/zh 文案同步。

**阶段 5 — GUI 美化 + 操作手感（用户项②④，主打高级感/现代感）**：视觉
令牌细化（间距节奏/圆角/层次阴影/图标一致性/克制的悬停与过渡反馈/空态）
+ 手感（快捷键覆盖、滑杆微调手感、缩放平移顺滑度、繁忙与进度反馈、toast
时序）；GUI 解禁截图自验，改前改后对比留档。

**阶段 6 — 16 路并行全扫收敛（用户令沿用上轮最重协议）**：16 个 codex
exec gpt-5.6-sol xhigh 各持一镜头盲扫全 crate（镜头重划，含本轮新面：
导出/手感/分模块后结构）→ 我逐条对源码亲证分拣 → 修复 → Codex xhigh
只读复审全 diff 至闭合；模型不可用如实上报，绝不静默换模型。

**阶段 7 — 收口**：clippy 0/全测试/i18n 审计/字体门；E2E 真进程（含真机
GUI 与导出路径——原生另存对话框本体不可自动化，验收=对话框以下的导出
内核函数直测 + 我截图目验）；五文档+记忆+ccm 同步；推送+发布 **v0.23.2**
（资产字节验证）；轮末 GUI 启动禁令恢复。

### 在册清单（原「下轮队列」，= 本轮全清范围。来源=第十一轮 16 路扫描
### 104 发现中"逐条亲证属实但判超上轮最小变更"的登记簇，按主题合并，
### 括号内为路号#序号）

- **披露穿线（架构级，最大簇）**（L08 全 10 项 + L15#2）：write_xmp 的损失
  注记只有 2/7 调用方消费（Save-all/Analyze/粘贴/反推/api-analyze 丢弃）；
  不可读外部 XMP 折叠成"absent"静默中性开照；目录扫描部分失败静默；蒙版栅格
  失效仅 stderr 而画布仍显"启用"；style 索引不可用静默失效；Analyze 复合保存
  toast 过期后状态行夸大；openai 提案 clamp 摘要只到 stderr（strict schema
  不支持 maxItems，openai.rs:308 已验证 400）——需统一"警告聚合→UI 通道"，
  逐点补=打补丁反模式。
- **内存边界统一**（L02 全 13 + L07#6/#7 + L16#2/#4）：全部 store/config JSON
  直读改有界读取器（variants/pixels/recipe/中央 XMP/autoshop.local.json）；
  蒙版解码统一有界原语（serve Fill/Heal 掩膜、retouch、GUI 笔刷/精修烘焙路径
  绕过 256MiB 快照）；serve 上传流式化+聚合字节预算+体读超时；generative 预览
  全幅合成瓦片化；GUI 冷变体全幅解码上限+LRU；style 构建全局解码信号量；
  serde_json::Value 反序列化放大（窄结构体+visitor 上限）；rawler IFD 环上游；
  retouch plan_from_mask 组件 bbox 覆盖预算；解码上限×下游 bpp。
- **并发/TOCTOU**（L01 全 6 + L10#2/#6）：GUI batch 配方+像素源单锁快照（读-写
  竞态）+ .bak 恢复先于选配方；lightroom_sidecar 读后句柄身份重验；load_version
  全程 NoWait 锁；SETTINGS_LOCK 跨进程化（GUI/serve 双进程丢更新）；serve
  If-Match 配方修订前置条件（双标签互踩）；denoise 并发同目标输出竞态（staging
  自有文件）；config 损坏救援 unlink 竞态。
- **耐久性**（L03 全 6）：recipe/pixels/variants 三件套单代原子提交（当前
  Ctrl+S 三写之间每个 kill 点都是不一致态）；显式清除 clear_develop 事务标记
  先行；heal/蒙版二进制载荷 sync_all 后再让 JSON 引用；版本删除 .deleting
  标记；config/style 持久化走 durable replace（保 0600）。
- **XMP 保真**（L05#1/#4/#6/#7）：属性化开标签/带空格标签的精确字符串盲区
  （present-but-unparseable 须计损失）；merge base 权威性（store 胜时旧
  sidecar 蒙版不得复活）；RAW 内嵌 XMP 包导入（最低优先级源）；命名空间以
  URI 而非 crs 前缀识别。
- **跨面一致**（L13#1-#4）：GUI batch 用共享 resolved-develop 加载器（现只认
  recipe.json，丢 XMP-only 显影）；has_develop 认 LR sidecar（batch 重复计费
  隐患）；Analyze 备份把更新的 LR sidecar 一并快照；produce_recipe 标定经
  newest-intent 解析（旧 recipe 的 base_curve/WB 锚污染新画布）。
- **GUI 状态机**（L06#2-#6）：冷母版按画布分辨率解码（现全幅 9504px 上桌）；
  蒙版选择切换取消武装中的 Redraw/加组件/笔刷；retouch 落地推迟到手势结束再
  按序提交；笔刷会话随底片替换取消；变体缩略图无完成帧时失效。
- **sidecar 加固余项**（L11#3-#7）：SCUNet 权重 SHA-256 钉死+旧 torch 拒绝
  回退 pickle；下载字节上限+.part 回收；claude.rs 复用有界 drain（现无界
  Vec）+ 进程组/Job 罩后代 + prompt 走 stdin（Windows 32767 argv 上限）。
- **测试质量余项**（L14#2/#3/#5/#7）：付费 POST 恰一次的注入测试（现测试
  grep 旧标识符名）；ClaudeProvider argv 隔离断言；Windows junction 循环
  夹具（现 fixture 建不成时空转绿）；瓦片规划器过公共入口测试。
- **i18n 余项**（L12#2/#3/#4/#6）：verdict/deterministic rationale 结构化后
  渲染时本地化；非 CJK 脚本动态文本（阿拉伯/泰/韩文件名）回退字体；worker
  闭包语言捕获改返回类型化数据；audit 校验中英占位符多重集对齐。
- **渲染数值**（L04 全 3）：DNG ColorMatrix 奇异/AsShotNeutral 零分量守卫；
  CA-only 时 fill scale 计入通道因子；style 索引有限溢出（f64 距离+物理界）。
- **CLI 余项**（L09#1/#4/#7）：付费调用前输出路径预检；heal --full-res 帮助
  文本对烘焙输入的降采样说明；subset_gui_fonts.py 捐赠数勘误+先验后写。
- **Unix 专项**（L16#3）：GUI 线程存在后首次 Config::load 的 setenv UB——
  .env 解析改 owned map 不动进程环境。
- **结构（我的架构审登记）**：Opened 恢复块（历轮缺陷热点）抽纯函数；
  gui.rs 巨石分模块（1781 处 self 引用）；read_saved_develop 4 元组、
  save-all 6 元组结构体化。
- **C1/F10 词法路径身份**：~~维持既有取舍（source.txt 面包屑），仅登记~~
  → 用户翻案（2026-08-09 拍板），本轮实修，见上方阶段 3。

## 第十三轮计划（🔒 用户锁定 2026-08-12 —— ✅ 已完成，v0.23.3；执行记录见「当前状态」首条）

> 用户原话（2026-08-12）："1.改应用图标。按照我们的风格改成类似恶搞adobe的PS的
> 图标，我举个例子：'skymanbp's AS'。2.在合适的地方加入一些彩蛋，讽刺Adobe的
> 拉跨技术和昂贵定价。3.看截图，GUI这里的按钮上下有点挤，稍微分开一点点？然后
> 下面的小按钮框框上下太短了，拉大一点？4.全量更新文档，彻查漂移。定版0.23.3。"
> 追加："roadmap记得全部清完。更新并锁住ccm计划，然后根据计划全部推完。"

1. **图标改版**：恶搞 Adobe Ps 的 "As" 瓦片（同款深藏青 #001E36 + 亮蓝
   #31A8FF 字母，Noto Sans SC OFL 600 字重，4096px 超采样降出全尺寸资产）。
2. **彩蛋**：讽刺 Adobe 定价/技术，i18n 双语，藏而不扰。
3. **GUI 垂直间距**：顶栏按钮上下留白 + Library 小按钮框拉高（溯源根因修）。
4. **ROADMAP 在册清完**：唯一可行动登记项 R12-03（xmlns 作用域化）实修。
5. **文档全量漂移彻查** + Codex xhigh 只读复审 + 发版 v0.23.3。
   **GUI 启动禁令本轮全程在场**（视觉验收=用户自己的截图）。

## 第二十二至二十四轮计划（🔒 用户锁定 2026-08-17 —— 用户 17 条反馈三轮清账）

> 输入：用户试用一周后的 17 条反馈/bug。产出链：双模型独立工作流分析（各 13 代理：
> 6 诊断+6 对抗核查+1 批评）→ 7 代理逐簇交叉核对（分歧读实际代码裁决）→ 与另一
> session 独立计划比对 → 用户 AskUserQuestion 拍板 8 条。完整技术档（全部 file:line
> 证据、分歧裁决、实现警戒线）在本地计划档 `feedback17-final-plan-r22-r24.md` +
> `feedback17-xcheck-consensus.md`（不入库；实现工单以其为准）。
>
> **用户拍板 8 条**：①分三轮发布；②不做「保存即自动快照」（改出处标注+可命名+
> 一键存档——claim_version 豁免删除指纹压制，自动铸号会让已删内容以新号复活）；
> ③蒙版去雾/清晰度/纹理上引擎、接受旧片重渲染；④参考图库 style-index 与图片参考
> 都要、透明度（在参考哪个库/哪几张）同级；⑤反推「参考成片」=**同帧另一次显影**
> （放开 GUI 入口+同帧性检测）；⑥反推 judge 闭环=**勾选式**（默认路径维持 v0.26.2
> 注记定案，勾「深度反推」才移到落盘前有界迭代，止血修先做）；⑦强度滑杆默认
> 0.65；⑧蒙版滑杆 LR 三组分组。

- **R22（v0.28.0）缺陷+UI**：#2 蒙版精修报错根修（load_image 入口拒 RAW+统一
  source_pixels 门收编 5 处手抄分派+masks.rs 走真显影+sidecar 缺失能力披露）；
  #17 缩略图恒定槽位+保比内嵌（方向链本就正确）；#9 画笔滑杆进蒙版会话行（共用
  self.brush）+孤儿会话修；#15a/#10 蒙版 clarity/dehaze/texture 上引擎（同批
  engine_active 两消费点+文档回写）+面板 LR 三组化+统一导出侧有损披露；#4/#14
  AI 面板整合（ai.rs，editable 门重建+R19 宽度算术随行+测试宿主同改）+尺寸令牌
  FIELD_W_MAX+分组弱标题+裸挂画笔收编；#11 导出分裂按钮+「目标」升一等设置+
  绝对路径显示；XMP 导出到照片旁按钮；打磨批（●指示器/消歧/保存侧披露）；
  先行 schema↔recipe 同步测试（含 LocalAdjustment 镜像）。
- **R23（v0.29.0）AI 大修**：单一控件注册表派生 schema/提示词/eval/style 四下游
  （lens 三项进 schema 必须连带「未表态」表达，防 Refine 覆写陷阱）+suggested_tools
  只建议不执行；风格库 GUI 建库入口+三条静默臂修+近邻披露+图片参考开关（先于强度）；
  强度轴 grade_strength 一次穿六道闸（提示词/temper 软帽/校验器/judge/风格二值门拆/
  启发式，白点保护耦合不缩放）；思考模式（in-schema 有序思考字段先实测生成顺序+
  多轮 judge 收敛 target/cap 随强度+batch/eval 恒关+成本按真数披露）；eval/style
  扩尺子（hsl/分级/RGB 曲线）；反推大修（入口放开同帧+诚实性批修 4 处+联合值域桶
  第二读数 fail-open+参数域 RangeMask→分级→WB delta→HSL 条件解+勾选式深度反推
  闭环=正式修订 v0.26.2 注记定案）。
- **R24（v0.30.0）数据模型**：「一张照片=底片+N 变体+版本历史」词汇+is_parametric
  谓词收归；variants.json v=1 加 id/name（六点搬运）+版本名 .version-meta.json
  咨询边车（照 .deleted-versions.json 非世代纪律，名字随号删弃+hwm 不复用合约钉）；
  应用到原图（Fitted 覆盖+一步撤销，Generated 禁用指路反推）+Generated 载回
  Original 的标定补盖缺陷修；variants:Keep 四写入点+批量粘贴旁路收敛、delete_variant
  墓碑、list_edits 统一视图、变体条+Versions 合并列表；LR 缺口五档 tier 注册表 M0+
  分批 B2-B5（依赖用户 LR sidecar 一次取证）+INERT_LOCAL 放宽+./out 投递根统一。
- **素材请求**：反推新复现对（RAW+参考成片）；LR 采样 sidecar 族（已知角度径向/
  组件组合/局部点曲线/AI 画笔蒙版，一次实验四项取证）；f944ef3 147 张对照集路径
  确认；#2 报错 toast 原文。
- **每轮硬门**：clippy 0、全量测试绿（基线 449+7+73+2）、audit_i18n 0、zh 词条过
  成品字体子集（口径见下方 v0.31.0 条的更正）、禁启 GUI、禁 cargo fmt、五文档
  +记忆同步、Opus 5 对抗复审后发版。
- **发版链 + 环境门套件（R27 L-29 补入，2026-08-19）**：
  `bump → 文档门 → feat → push → tag → gh release → assets 回下载逐位 cmp →
  台账章`（文档门=`python scripts/check_docs.py --gates <电池转录>`，2026-08-20
  起，11 行注册表，FAIL 即停；台账章同时把 gitignored 的 M1_PLAN/V2_PLAN 快照到
  `~/.claude/plans/autoshop-ledger-snapshots/<日期>/`——它们 2026-08-20 起不进远端，
  git 不再为其留版本）。
  **每次发版必须另跑 env 门控的真实夹具套件并把张数记在门数字旁边**：
  `AUTOSHOP_LR_PROBE_FIXTURES`（真实 LR sidecar 往返，v0.32.0 实跑 16/16）、
  `AUTOSHOP_MB_FIXTURES`（M-B 采样族）与 `AUTOSHOP_RAW_ZOO`（CC0 多机 RAW，
  R27 Batch-1b 起，`decode.rs`；过滤串必须用测试全名
  `every_make_in_the_raw_zoo_decodes_and_agrees_with_itself`——v0.33.0 电池用
  `raw_zoo` 简称 0 命中的教训）。**未设变量则静默跳过**，所以裸 `cargo
  test` 的门数字（620/8/129/2+2 这一族）**不含**它们——真实边车往返出回归也能
  绿着发出去，这正是 L-29 登记的风险。记法照门数字：「620 lib / 8 CLI / 129 GUI
  / 2+2 合约 + LR 探针 16/16 + M-B n/n + zoo n/n」。同族同样静默的还有需要真 RAW 的
  `AUTOSHOP_PROBE_RAW`（`render.rs` / `lensmeta.rs`）与 `AUTOSHOP_ORIENT_PROBE_RAW`
  （`decode.rs`）——有真机时一并跑并记数。`AUTOSHOP_THINK_PROBE_KEY`（付费）
  不进硬门，按轮决定。

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
| **M-C** | `f944ef3` 那 147 张 RAW+xmp 对照集是否还在、路径确认 | ✅ R25 全量重跑（数字见 v0.31.0 条）→ **R27 第三次尝试落地成新基线（2026-08-19）**：HEAD `f43dd85`、中转端点（gpt-5.6-sol）+ `--jobs 3` 并行、147/147 全 done、**双流零回退**、约 38 分钟；`~/.claude/plans/r27-materials/mc-eval-147-r27.txt` 自此为唯一参照，R25 降为历史。前两次尝试在案：#1 死于 83/147 commit 内存墙（Batch-7 已根治）、#2 有 2 条中转上游 `stream_read_error` 回退按判据废稿。头条：**gap 15.5%；蒙版拒收 2.35→0.05 张/照（Batch-4 导入根修实证）；whites 偏置 +22.55→+16.20、blacks −14.18→−10.87** |
| **M-D** | #2 蒙版精修报错的 toast 原文（分流哪条臂） | ✅ R24 两臂都修 + 实跑否证 |

## 当前状态（已完成，勿重做）

- **R28 Batch-3：AI 蒙版帧身份 + rotate 文件系统原子性（2026-08-20，未发版）** ——
  3a（F1 两根一修）：resolve_ai_masks 改双参 `(raw=身份, src=像素)`（F1-C：两调用点
  曾把烘焙母版当唯一参，键与缓存家落在不属于任何照片的 develop 目录）；
  stage_source_frame 改 `decode_any_turned(src, recipe.quarter_turns)`（F1-B：帧
  从配方取非从盘取，与 render 两臂同门同折，烘焙母版从「解码失败披露不渲染」变
  「正常解码渲染」）；**ai_cache_key 加帧项** `…|t{turns%4}|…`（F1-A：(0.5,0.5) 不动
  点修死；%4 与 render 折叠对齐防手改 5 铸双键）；staging 名加 AtomicU64 序号
  （F1-D 潜在碰撞，对齐 style.rs:400 形制）。**行为披露=v0.33 及以前的全部 AI 蒙版
  缓存换键一次性重算**（秒级 GPU/蒙版，经 describe 通道如实报告；旧 PNG 留盘=既
  接受的 superseded 残留）。3b（F8）：**根修=集合错配**——新 turnable_raster_paths_mut
  （只含 Bitmap）与 bitmap_paths_mut（存储集）成为共享 geometries_mut 走查上的两条
  成员规则；phase1 跳 AI 缓存；错误路径删已转副本+turn_raster_file 释放未填 claim
  （对齐 store::detach_rasters/gui masks.rs 两先例）；GUI「nothing was changed」
  toast 对盘面也成真；rasters_turned 只计实际重指的。**附带行为变更（复审裁可）**：
  AiMask raster 指向缺失文件曾整转失败，现按旋转语义跳过清引用。store.rs 文档=
  消费者 4→3、「stale thumbnail 非 wrong pixel」句因第四消费者移除而重新为真+
  留规则句（要渲染帧必须从手里的配方取）。门（主审复跑）=clippy0×2+**711**(+4)/
  11/131/2+2+zoo 全名 9/9；变异三组（代理）+帧项变异主审亲手复现。**未验证登记**：
  turn_raster_file 的 claim 释放支无确定性触发法（构造性证明+全目录不变式兜底）；
  AI 路径无端到端实跑（帧相等性为代码论证：decode_any_turned 与 render 两臂同折）。
  **代理过程自纠**：中途 git checkout 误废自家 segment.rs 编辑，重施并复验（diffstat
  前后一致）——后续变异改用 scratchpad 备份。ARCHITECTURE AI 蒙版键描述同批补帧项。

- **R28 Batch-2：防御性收口五项（2026-08-20，未发版）** —— 2a **无界 store 读类**：
  saved_quarter_turns 改 read_text_capped+单字段 SavedTurn（无 deny_unknown_fields
  =前向兼容保留、%4 与 clamp 对齐）；兄弟 adopting-from.txt 两处改 64KiB 专帽
  （Windows 路径上限论证）；**扫描门顺手抓出第四处旁路** export_xmp_beside 一并收口；
  新源扫描门 every_store_read_goes_through_the_capped_reader（扫 store.rs+segment.rs
  前向哨、两条前提断言防空转）；文档撤回「错的代价只是陈旧缩略图」句（segment.rs
  第四消费者证伪它，指针指 F1-B/B3）。2b **笔刷帽根修**：cap_tokens 按 '\n' 界切
  （帽=上限非精确尺寸）；cap_strokes 共享 Brush/AiMask 两臂；截空整笔丢弃并计数；
  xmp_to_recipe_clamped 新门（薄包装 28 调用点不动）接三披露面=CLI 第三句+GUI 三
  restore 分支+export（**零新字符串**=复用既有本地化键，无 cmap/i18n 暴露）；
  dab token 加 256B 长度界。2c **check_docs 加固**：块注释跳过+_PROBE 加案例+
  [&str;N] 交叉核对（含失败向自检 _raises_lookup）+括号计数前遮引号段（_TOML_PROBE
  合成自检）。2d jobs.rs saturating_mul+u64::MAX 邻域测试。2e **三注释谎言改真**：
  style.rs:559/575（citing git -S bae6d99、真修指 B4-4b）+main.rs:1431 限定到
  stdout（stderr 站点按 HEAD 重枚举、指 B5-5c）+store.rs 文档随 2a。
  **裁决档案勘误（代理实测）**：F5 加重项示例「30 万整数位」错——溢出成 inf 早被
  有限性检查拒收，可达形是小数形 0.111…（fixture 与注释已按实况记录）；F6 引行
  render.rs:1133/2786 因 B1 移位失效，注释按 HEAD 重枚（:1430/:3083）。
  **代理自纠**：cap_strokes 首版插错位吞掉 cap_geometry 文档块——读自家 diff 抓出
  非门抓出。**具名不改**：serve.rs 三处+store.rs:3907 留 xmp_to_recipe（无披露面
  可喂）。门（主审复跑）=clippy0×2+**707**(+5)/**11**(+1)/131/2+2+zoo 全名 9/9；
  9 组变异证据（代理）+saturating 变异主审亲手复现。cap() 13 调用点普查=2 改
  11 留（全非 token 域）；read 普查=4 改 2 惯用法留 14 read_dir 留。
  README CI 句精确化（every push→push to main+PR，Show HN 材料代理发现）同批。

- **R28 Batch-1 1a+1b：X-Trans 色偏根修 + 两披露句改真（2026-08-20，未发版）** ——
  确诊=rawler PPG 色度插值按 Bayer 公理从「右+下」邻取 R/B（ppg.rs:185-203），
  X-Trans 每 36 像素的四个 2×2 全绿块使其失败 16 次：8 相位 R 从未被写（=0）、
  另 8 相位 B=0（实测 99.8% 像素；余 0.2%=上游 3px 边环规则）；WB 对角阵与
  对角缺失可交换故不可修复（诊断代理+主审亲读 ppg.rs:185-203/render.rs:396/
  0 字节 xtrans_markenstein.rs 三证）。修（用户拍板选项 b）=render.rs 内
  CFA 几何感知去马赛克（+600/−57 单文件）：自家光位通道原值保留；缺失通道按
  (相位,通道) 预算 5×5 **平面拟合**权重回填（半径 1 时 108 组中 56 组样本<3、
  均值回退在线性梯度上留 7.2e-3 每相位色差→平面拟合 3.3e-16；全 108 组权重
  非负=凸组合不振铃，阶跃过冲实测 0.0000）；出帧 tap 按整 CFA 周期回折非镜像；
  分派与披露共用一个几何谓词（cfa_needs_geometry_demosaic）。**实测收敛：
  X-S10 全帧 G/R 1.5503→0.9476、G/B 2.0839→1.0313（八格式带 0.81-1.08 内、
  相机预览 0.95）；每相位散布 R 157.80%→0.34%**；八个 Bayer zoo 渲染 FNV
  哈希前后逐一相同（2×2 分派字节不变佐证）、RAF 两次渲染哈希相同（PPG 在
  6×6 上的 Color2DPtr 并行竞争顺带按构造消除）。1b=X-Trans 披露句改真（旧
  「Colour…unaffected」src/ 清零）+BrushCarried 两处「measurement in flight」
  →「measured kernel has no closed form + pre-lens-correction frame」
  （xmp.rs:1433/1664；钉测试只查 not yet rendered 保绿）+gui/util.rs 注释同步。
  门（主审复跑）=clippy0×2+**702**(+7)/10/131/2+2+zoo 全名 9/9（91.35s）；
  变异主审亲手复现（去 ROI 相位移位→flat 测试红 0.405≠0.2→还原绿）。
  **未验证登记**：细节质量无参考转换器对照（合成量尺+披露句代偿，待人工目验）；
  2×8/12×12 几何仅构造性证明；README 格式条 raf.jpg 仍为 v0.33.0 修前渲染=
  发版批重渲。README 展示节/§X-Trans restated/ARCHITECTURE 拒收表同批改真。
  诊断档案=~/.claude/plans/r28-materials/xtrans-diagnosis.md。

- **R28 修复轮已锁定 → v0.34.0 全五批（🔒2026-08-20 用户六拍板）** —— 拍板：
  ①范围=**全五批**；②RAW 单文件内存=**与 baked 同款 4GiB 峰值闸**（超限拒收+具名
  披露）；③XMP 作用域=**连 Scope newtype 类型化大改**（tag/subtree 语义上类型，
  非只换 crs_attributes 读器）；④跨平台编译验证载体=**GitHub Actions**（ubuntu+
  macos cargo build，结果链接入 README；workflow 本日落库随本条同批提交）；
  ⑤（宣传联动）取证博文标题=**标题二**（four numbers not a bounding box）；
  ⑥README 加 **clean-room 政策声明**（落 Batch-1，博文引用它）。批次划分：
  B1 宣传解锁（X-Trans 通道映射根查+修/两假披露句/CI/clean-room 句）→B2 防御收口
  （无界读类 store.rs:1219+865/920+源扫描门/笔刷帽 token 界+ClampSummary 披露/
  check_docs 加固/jobs.rs:137 saturating/三注释谎言）→B3 帧身份（AI 蒙版 staging=
  decode_any_turned(recipe.quarter_turns)+缓存键加帧项+身份归 raw+staging 序号/
  rotate phase1 跳 AiMask+错误路径清理）→B4 资源预算（单文件峰值进准入+RAW 4GiB 闸
  +延长 Batch-7 探针复测 render 尾巴/GPU sidecar --fp16+模型单飞或 manifest+GUI 退化
  披露+DecodePermit 不扣穿）→B5 语义口径（texture−100 设计项/shadow_hue 口径/诊断行
  盖 stem+batch 不弃 _notes/XMP 类型化）。计划全文（已盖拍板）=
  `~/.claude/plans/r28-plan-proposal.md`；Codex 九根因证据=同目录 codex-r27-review-
  adjudication.md。预计零 recipe schema 变更；3a 缓存键一次性失效（重算披露）；
  1a 为渲染行为变更入发版说明。

- **Codex gpt-5.6-sol 只读审阅 + 十发现全裁决（2026-08-20）** —— 审阅基线
  012cb60（代码域 src/scripts/python/Cargo.* 与 HEAD 字节同；rollout 文件证实
  gpt-5.6-sol/xhigh/只读沙箱，33m49s，porcelain 双证零写）。回传 2H/5M/2L/1N、
  零 CRITICAL；十条经 10 个 Opus 对抗核实代理逐行验证 + 主审亲读关键行后裁定：
  **0 fix-now / 9 登记 R28 / 1 note-only**。两 HIGH 均降档：F1 AI 蒙版帧身份
  →MEDIUM（站点 D「同名并发污染」证伪=全树无并发调用者；A 仅正中心点 (0.5,0.5)
  可达；B=外部配方 quarter_turns 与库不一致才出无声错像素——唯一带成功报告的
  错像素臂；C 已披露非静默、根在调用点非 resolver）；F2 计划器 1800MB
  →MEDIUM-HIGH 且真根更深：常数无量纲、RAW 路径根本无单文件上限（150MP IIQ
  默认 --jobs 3 比 Codex 的 baked 场景更糟、零 opt-in），decode.rs:350 单文件
  峰值已存在但从不进计划器；1771MB 实测只覆盖 produce_recipe，batch --render
  全分辨率尾巴未测=待延长 Batch-7 探针复测。核实另掘 Codex 未列同类兄弟 7 项
  （store.rs:865/920 无界读、三处注释谎言 style.rs:575-578（git -S 证生来就错）
  /main.rs:1431-1433/store.rs:1204-1217（消费者 2 实为 4）、dab token 无长度界、
  --fp16 未接线+DecodePermit 扣穿 sidecar、segment staging 名缺序号、RAW 无上限
  变体）；Codex 两处引行有误已更正（F9 真源=check_docs.py:200-201）。逐条证据
  （file:line+反驳尝试+上游根评估）：`~/.claude/plans/codex-r27-review-adjudication.md`。

- **发布批 P4 收尾三轨落地（2026-08-20）** —— ①**eCO 定稿**：申报草稿占位符清零（v0.33.0 / 首发 2026-08-19 / Domicile=United States 用户确认），Circular 61 交存 PDF 已生成（源码首尾各 25 页 / 全listing 1854 页 / 51 页 PDF 经 pypdf 验证+50 页逐行比对零错位；**交存内容取自 tag v0.33.0 而非工作树**——HEAD 领先 4 个 src 注释文件，交存须与登记版本逐字节一致=主审裁可），逐屏填表指南就绪（费用不编造、以付款页为准）——只欠用户上 eCO 网站填表付费；②**GitHub 完备化**落地（8556e7f：bug/feature 双 issue 表单（披露行一等字段）+ SECURITY.md（私密通道）+ PR 模板=真实发版电池清单；Discussions 已开 has_discussions=true）；③**目检包**建成 ~38MB（10 条登记项全带台账出处：Duv/λ限幅/kelvin 接缝/M-C 判读/L-12/常数极值/L-18/W_EMB/L-13/subject 三案——第三案 BiRefNet 码内不存在=阻塞如实标注；26 配方渲染+24 裁片+5 全分辨率差分图；旧引擎复现=解锚点使增益恰等旧系数 f32<4e-8 非模拟）。**目检包顺手三发现**：ROADMAP:2944 暖侧数字系台账错误（同日勘误）、texture=−100 端点=彻底高斯糊非 LR 保边语义（σ−92%）、eval shadow_hue mean|Δ|=141 是 sat=0 不查饱和度的记账假象（真事实=AI 加阴影染色 sat bias +9.02）——后两条入 R28。
- **发布批 P2 README 展示重写落地（2026-08-20，用户拍板=混合图源、三组全批、不规则水印、RAF 保留诚实标注）** —— README 全量重写（14 节：闭环叙事 / 三组 before/after（用户照片缩 1200px，含 524 退化被验证人拒收的失败案例、两组「天空比中性版发白=全局提亮盖过天空蒙版」反例如实标注）/ 九格式 CC0 条带 / 147 对实测数字逐字引台账 / 诚实模型披露表 / 信任模型 / 完整命令与 env 表；check_docs 两锚点保留）。对比图保护（用户令）＝**不规则水印**（组间角度/位置随 stem 哈希 22–38°、组内 before/after 一致不扰对比）+ EXIF Artist/Copyright + Credits 版权保留声明（**照片不随 MIT**）；入库 docs/images/ 16 图 ~1.9MB，EXIF 隐私核=零 GPS/序列号。**新缺陷入 R28：X-Trans 通道映射色偏**——RAF 中性显影 G/R=1.55 vs 相机自带预览 0.95（其余八格式 0.81–1.08；as-shot WB 读取正确）⇒ Bayer-on-X-Trans 疑通道错排，且 render.rs 披露句「颜色不受影响」被实测证伪，README 按诚实模型照登（「X-Trans, restated」节）。另录：CLI 无 export-at-size（README 如实脚注，候选功能）；渲染沙箱=AUTOSHOP_DATA_DIR，真库/真配置零触碰。
- **发布批 P3 仓库瘦身（2026-08-20，用户令「清理脚手架/过程/过时/测试代码，M1_PLAN 之类撤出远端」）** —— 普查 92 个跟踪文件：**无**脚手架/草稿/一次性调试物（src 51 全产品码、scripts 3 全发版门、tests 2 全合约回归）；「过程」类=三份台账。用户三拍板：**只撤 M1_PLAN+V2_PLAN**（ROADMAP 留远端）、**原地留盘+gitignore**（路径不变、代码注释与台账纪律照旧；git 历史仍含旧版，未洗史——洗史需重写+强推会断已发布 tag，且两档无秘密）、**测试全保留**（695 单测内嵌 src、发布构建本不含）。机制=`git rm --cached`+.gitignore 两行；ARCHITECTURE:1197 加「私档」注记；src 约 30 处注释引用两档=既有私档引用类（PROBE3-ADDENDUM 先例）不动；README 的一处引用随 P2 重写落地时处理。**台账章新增步骤（见发版链）**：把两份 gitignored 台账快照到`~/.claude/plans/autoshop-ledger-snapshots/<日期>/`（首份快照 2026-08-20 已存）。
- **v0.33.0 已发布（2026-08-19，R27 全轮收口）** —— 内容=R27 十个批次：
  Batch-1a 台账清账六转换 / Batch-1b 多格式（RAW 24 扩展+烘焙 8；CC0 九机 zoo）/
  Batch-2 90° 旋转骨架（quarter_turns）/ Batch-3 像素战役几何根修消费 /
  Batch-4 笔刷蒙版一等携带+解析器加固+许可证根修 / Batch-5 SigLIP2 风格嵌入+
  AI 蒙版本机重算 / Batch-6 估计器验证（M1 #12 关、gray-world 证伪）/
  Batch-7 `--jobs` 内存预算并行+embed 撞名根修 / Batch-8/9/10 笔刷径向终局
  实验（合成 sidecar 路线）+ `LR_MASK_FRAME_SCALE 1.032→1.0`（用户拍板）。
  **⚠ 前向断裂（发版说明置顶）**：v0.32 exe **硬拒**带 quarter_turns/笔刷蒙版/
  AiMask 的 v0.33 recipe（deny_unknown_fields 响亮拒收）；不带这三者的 recipe
  字节不变双向可读。**行为变更**：径向蒙版按 sidecar 实存几何渲染（旧值整体
  放大 3.2%=单帧镜头畸变误当常数）；batch 转录按 index 序。
  **门（发版电池实录）**：clippy 0×2；**695 lib（8 ignored）/ 10 CLI / 131 GUI /
  2+2 合约** 双配置；**LR 探针 16/16** 径向 sidecar 字节往返（canonical 集）+
  实验 5 份另过；**M-B 7 份 42/42 蒙版导入 0 拒收**；**RAW zoo 9/9**（X-Trans/
  ORF 披露按设计触发）；字体 cmap 803/803；audit_i18n 0/0。exe：cli 30,668,634 B
  / gui 40,290,588 B（tag 后回下载逐位 cmp，哈希入记忆台账）。
  发版说明全文：`~/.claude/plans/r27-materials/v0330-release-notes.md`。
  feat/tag=`000444c`；assets sha256：cli
  `e670f91e6480db6f639ced36af802d0c8cf899a8e9e34c022bd4e1e4e2f11db2` / gui
  `6c69ee3f71a2bb31e14da108b55f76a523577ea533c084362527ba9b452e00b8`
  （回下载逐位 cmp 通过）。**电池勘误（2026-08-20）**：转录 `v0330-gates.txt` 的
  「=== RAW zoo ===」块存的是过滤串失配的那次调用（`raw_zoo` 简称 0 命中、703
  filtered out）；全名 `every_make_in_the_raw_zoo_decodes_and_agrees_with_itself`
  当日另跑 9/9，发版后独立复跑再证 9/9——照抄该块会把「没跑」读成「过了」，
  下轮电池必须用全名（发版链一节已钉）。**待办**：发布四件套（eCO 定稿 /
  ~~文档门~~ ✅ `scripts/check_docs.py` 2026-08-20 落地 / README 展示重写 /
  GitHub 完备化）+ 目检包。**R28 候选**：`.lcp` 解析（笔刷渲染帧 + 径向逐帧
  保真的唯一根修）、羽化两支替换法待判（含 L-06 `d_out`=1.64 与 Batch-10 饱和
  ≈1.41 两测未调和，见 V2_PLAN §7-1，勿自行脑补和解）、Feather→CenterWeight
  映射（仅 3 点）、κ 跨镜头/半径/硬度普适性、`BrushCarried` 披露文案更新
  （「kernel measurement in flight」已过时——测量已落地，堵渲染的是核无闭式+
  镜前帧；同批还有 X-Trans 披露句「颜色不受影响」被 P2 实测证伪，两句一并修）、
  **X-Trans 通道映射色偏根查**（RAF 中性显影发绿发暗，证据见 P2 条目）、
  **texture=−100 端点语义**（实测彻底高斯糊 σ−92%、非 LR 保边中频平滑；材料=目检包
  crops/D-texture-m100__branches.jpg）、**eval shadow_hue 口径修正**（eval.rs:664-665
  任一侧动了就计入但不查饱和度，sat=0 的 hue 无色仍记差致 141° 假象——修口径非修数字）、
  **Codex 审阅九根因（2026-08-20 裁决；逐条证据见
  `~/.claude/plans/codex-r27-review-adjudication.md`）**：①AI 蒙版帧身份（缓存键
  无帧项+staging 用盘上 saved_quarter_turns 非配方值；修=decode_any_turned(src,
  recipe.quarter_turns)+键加帧项+身份归 raw，一修闭 F1-B/C）②计划器无量纲
  1800MB（单文件峰值 decode.rs:350 已存在不进计划器；RAW 无上限变体更糟；
  jobs.rs:137 saturating 搭车）③GPU sidecar 无 VRAM 预算（RAM 形状预算嵌套
  GPU 加载；manifest 批处理死代码+--fp16 不传+DecodePermit 扣穿+发布版 GUI
  全败仍成功 toast）④XMP 作用域=惯例非类型（crs_str tag/subtree 过载+
  crs_attributes 只认双引号→单引号文档 11 来源键静默丢）⑤笔刷帽单位错配
  （tokens 65,536 vs bytes 256KiB+字节切断 token 破坏存储不变式+ClampSummary
  xmp.rs:6586 落地即弃+writer 原样发坏 token→下次打开整组消失）⑥诊断行无
  身份（无调用方 sink+batch main.rs:1550 弃 _notes+main.rs:1431-1433 假注释）
  ⑦无界 store 读类（store.rs:1219+兄弟 865/920——capped reader 24 调用点的
  仅有旁路）⑧rotate_recipe 文件系统原子性（错误路径孤儿无回滚+phase1 集合
  错配含 AI 缓存；成功路径孤儿证伪=无发货写入者）⑨check_docs 词法器加固
  （块注释盲+括号计数不遮字符串；当下 012cb60 无触发已逐行验；修=_PROBE 加
  案例+[&str;N] 交叉核对+遮引号段）。

- **第二轮实验消费：流量律关闭、两定律再反转、L-05 机制定案（2026-08-19，
  R27 Batch-9+10，零码变更）** —— 用户「操作想吐」后改**合成 sidecar** 路线：
  Batch-9 把 17 个实验状态写成 ARW 副本+外科手术 xmp（五道验证门 17/17；
  staging `autoshop-fixtures/lr-brush-round2/`），用户只做导入/全选/导出三动作，
  **LR 全盘接受合成 Dabs**（实渲染验收）。Batch-10 消费（报告 593 行
  `batch10-report.md`，中途一次 API 服务器错误断点续跑无损失）：
  **① 流量律 CLOSED**：沉积可分离 `D(f)·k(ρ)`，`D/(1−D)=κ·f/(1−f)`，
  **κ=0.1219±0.0027**（拟合 rms 0.0070、留出 0.0097；`D(1)=1` 零自由参数
  =杀死「flow 1.0 走不同代码路径」假设；Batch-8 的不可分离结论=id 合成读法
  +零点两个伪影，已给机制解释）。screen 累积+浓度缩放**样本外再证**
  （51 dab 留出笔画 rms 0.0045）；12 条留出笔画 8 条进量化底。
  **② 合成律再反转：`par` 非 `id`**——同一蒙版四幅度阶梯自洽性 par 0.0057 vs
  id 0.1048（12×）；自由幂返回 p=0.786 且更差=被识别非拟合。**本引擎的参数
  缩放合成反而与 LR 一致**；Batch-8 的「局部 −1≈全局 −0.66EV」随 id 一并撤回。
  **③ L-05 机制 CLOSED：Adobe 镜头档案畸变**。开关对把隐含标度 0.98396→
  0.99826；11 个离散 dab 的位移**纯径向** `dr=−0.02487r+2.285e−9r³`
  （rms 2.94px；纯标度 11× 证伪、k=1.032 30× 证伪）。「径向均匀性」反证消解
  （窄环带上畸变多项式局部≡标度）。**结论：sidecar 几何存于 plain 帧
  （关档案实测 0.998），LR 在镜头校正前栅格化蒙版，`LR_MASK_FRAME_SCALE=1.032`
  两臂皆错**——三选项上呈后**用户拍板改 1.0（同日落地）**：按 sidecar 实存
  几何渲染、Adobe 畸变登记为未建模（.lcp 解析=R28 候选）。5 个钉 1.032 派生值
  的测试逐一按 k=1.0 手推重钉（其一目的消亡改写为「affine=恒等」并保留
  PROBE4 的 88px 畸变残差为披露史料）；门全绿 695/10/131/2+2 + 真实 sidecar
  往返套件（12 出口 canonical 集 + 5 实验集）全过=往返 k 不变性实证。
  **⚠ 行为变更（发版说明）**：每个导入径向蒙版的渲染几何改按存储值
  （旧值曾整体放大 3.2%），典型帧准了、强畸变帧（如 PROBE4 那类）残差
  =该帧自身镜头畸变（0–3.4% 实测）。
  **④ 羽化两端点皆错**：d_in=1−f 首次被直测证伪（exp_E_ref 解锁内半支：
  d_in≈0.79−0.94f，f=100 为负）；d_out≈1.41 饱和在 4.35 纵横比第二几何复现
  ——render.rs:2140 的 `ramp(1−f, 1+f/2)` 两端皆非实测，登记不动
  （与帧决策同一几何面）。
  **⑤ 渲染臂仍不实现**（预注册判据复评 batch10 §7.2）：流量过、**核形不过**
  （11 档无闭式，表插值 max 0.0344=4× 量化底）+**新阻塞=蒙版活在镜头校正前
  的帧**（本引擎无 .lcp、不读 LensProfileEnable、自家几何级用 Sony 多项式
  ≠Adobe 的）——帧问题未决就烘蒙版=在已知错的位置上烧像素，不如诚实的
  BrushCarried。实现草图（预栅格化如 Bitmap/AiMask、零 schema 变更、κ+11 档
  表钉测试）在 batch10 §7.4 备用。
  **未验证**：κ 单镜头单半径单硬度；畸变系数仅 B 帧；Feather→CenterWeight
  仍 3 点；apply_masks 与引擎几何级的先后未追。

- **笔刷/径向终局实验消费（2026-08-19，R27 Batch-8，纯取证批——一行码未动）**
  —— 用户按 v3 清单交付 16 JPEG + 5 xmp（仓外 `autoshop-fixtures/lr-brush/`，
  A 帧 14 组件与设计逐项指纹吻合）。报告 `~/.claude/plans/r27-materials/
  batch8-report.md`（593 行，预注册判据 §6.1），六项结论：
  **① 笔刷四轴钉死、一轴失败→渲染臂保持 Carried**。钉死：累积律=**screen 双轴**
  （单击剖面前向渲染 90 dab 长拖，峰值误差 0.4%；max 死、sum-clamp 偏置输 3.2×）；
  **浓度=对每 dab 沉积的缩放**（cap 读法 13× 证伪——P4 §7.1 错）；dab 坐标系
  plain-1.0 再证（7σ 排除 1.032）；0.2r 间距复现、波纹低于 8-bit 量化底。边角
  收获：**LR 笔刷 Size = α=0.5 直径**（266.1±5.0 px 跨羽化梯不变，`crs:Radius`
  是外支撑 276→733）；`CenterWeight ≠ 1−Feather/100`（f=50 → 0.1621）。
  MARGINAL：核形 `(1−ρ²)ⁿ` rms ~5× 量化底、n(h) 仅 3 点。**FAIL：流量**——
  单 dab 沉积 ≠ flow·k(ρ)（model-free 2.56× 证伪+长拖二证 2.8×；形状随流量变
  =不可分离；三峰无闭式）。按预注册判据不实现：naive 流量律在 flow 25 错 3.7×，
  比诚实披露更糟。**唯一解锁条件=§7 item 2 的 11 档流量梯（曝光 −1.00）+
  item 1 的原样参照导出**。
  **② L-05 裁决：拆分消解、常数被否**。一个**每帧标度 s**（对帧心）同时复现
  圆心+双轴 ≤2.2 px（在码模型差 30–104 px）；s = 0.984(24mm)/0.9996(105mm)/
  1.0044(34mm) vs 探针帧的 1.0315/1.0325——**k=1.032 是每帧量非常数**（离新均值
  3.4σ）。机制开放（镜头档案假设符号量级全对但被径向均匀性反证）。
  **`LR_MASK_FRAME_SCALE` 主审拍板不动**：无机制则无原则性替代值，任何常数都
  在某些实测帧上错；决胜导出=同一张照片 LensProfileEnable 0/1 各导一次
  （§7 item 4，5 分钟）。理由已写进 xmp.rs 常数注释。
  **③ Feather 全曲线：形确认、端点被否**。smoothstep 形 rms 达量化底、中点律
  1−f/4 存活；**d_out 在 f≥50 饱和于 ≈1.41**（测 1.223/1.402/1.406/1.414 vs
  律的 1.125…1.5）；d_in 只夹逼未测（缺原样参照）。引擎渲染不动——单几何证据，
  §7 item 8 的第二几何先行。
  **④ 幅度可分性 PASS**（α(−1)/α(−4)=0.2603±2.3%，6× 范围无漂移），并测得
  **合成律=对修正后输出按 α 混合（id）**非参数缩放（par 预测 −12% 漂移，缺席）
  ——本引擎按参数缩放合成，分歧首次量化。
  **⑤ 局部曝光≠全局曝光**：局部 −4.00 比全局 −4.00 深 3.8×、局部 −1.00 反而
  弱于全局 −1.00——算子形不同非标度差（仅两点；§7 item 7 建全曲线）。
  **⑥ 后续导出清单**=`batch8-report.md` §7（8 项，item 1 最廉价高值、item 2
  是渲染臂唯一阻塞、item 4 定 L-05 机制）。落账：xmp.rs 常数注释更新 +
  V2_PLAN §7 items 1/13 + 本条；未验证清单见报告 §8。

- **并行图片处理 `--jobs N` + 内存根因定案（2026-08-19，R27 Batch-7，~~未发版~~ →已随 v0.33.0 发布）**
  —— 用户令「尝试并行图片处理加速」。**先取证，后动手**：R27 Batch-6 的 147 张
  HEAD 重基线死在 83/147（~80 分钟，机器撞 commit 内存墙），本批把那次死亡溯到
  最上游并**证伪了「泄漏」假设**。

  **① 根因定案 = 无泄漏，是单张峰值（有数）**。两次第一方测量，零 API 花费：
  (a) `eval::tests::estimators_against_the_photographers_own_sliders`（`#[ignore]`
  取证探针，只解码不打网络）对**全部 147 对**跑 44.77 s，进程 commit 按四分位
  均值 112 / 127 / 115 / 122 MB、峰值 WS 254.5 MB —— **整段平坦，无单调增长**；
  (b) 一个 scratchpad 专用探针（不入库）复演 `produce_recipe` 的**非网络半程**，
  逐张读进程自身的 `PeakPagefileUsage`：**decode 半程峰值 commit 151 MB / 0.10 s
  一张**，**标定半程（`pipeline::photo_base_knots` → `render::render_to_image`）
  峰值 commit 1771 MB / 2.1 s 一张**。11 张连跑高水位只从 1771.3 → 1772.3 MB
  （+1.0 MB），张与张之间的常驻 commit 恒为 12.2–12.6 MB —— **泄漏会把高水位
  每轮顶高，它没有**。1.77 GB 全在 demosaic：`render_to_image` 先出**整幅**
  oriented f32（61 MP × 3 × 4 B ≈ 732 MB + rawler 自己的 sensor 缓冲 + 转向瞬态）
  **才**套 `max_edge`（render.rs:266-272），所以 `photo_base_knots` 那句「≤2048
  工作分辨率」只约束了 develop 各级，**约束不到峰值**。结论：要修的是**峰值 ×
  并发数**，不是增长。

  **② `--jobs N`（新模块 `src/jobs.rs`，std only，零新依赖）**。`batch` 与
  `eval` 各得一个 `--jobs`；**默认 = 各自改动前的并发度**（`eval` = 1 串行、
  `batch` = 3，即 R26 起写死的那个 `work.len().min(3)`），所以不带旗跑等于旧行为。
  worker 数 = `min(--jobs, 待办张数, 内存闸)`，内存闸 =
  `min(可用物理, 可用 commit) × 50% ÷ 1800 MB`（常数
  `jobs::PER_PHOTO_PEAK_COMMIT_MB`，出处即上面 1771 MB 上取整；50% 的另一半留给
  机器其余部分**和 `eval` 每张 spawn 的 `claude` 验证子进程**——那是一整个 Node
  运行时，**不在**这个常数里）。可用内存 Windows 走 `GlobalMemoryStatusEx`
  （windows-sys 加一个 feature，无新 crate），Linux 走 `sysconf(_SC_AVPHYS_PAGES)`，
  其余 unix 返回 `None` = **不设闸**（宁可不猜）。**被闸降级时打印一行披露**，
  静默降级会读成「旗子没生效」。**注意 `batch` 此前那 3 个 worker 是完全无预算的**
  —— 3 × 1.77 GB ≈ 5.3 GB 同时要，这是本批顺带堵上的既有隐患。

  **③ 输出纪律：按 INDEX 顺序，不按完成顺序**。`jobs::Sequencer` 收下每张的整块
  文本，只有前面全部落纸才放行第 i 块；worker **不阻塞**（轮不到就寄存，几百字节
  一块）。`batch` 原先是「完成即抢 stdout 锁打整行」——整行不撕裂，但
  `[7/50]` 会排在 `[3/50]` 前面，同一文件夹重跑得到不同顺序的记录；现在确定了。
  **诚实登记一条盖不住的**：管线深处的 `eprintln!`（如 pipeline.rs:390 的 GPT
  proposer 回退 ⚠）直奔进程 stderr，**按完成顺序出现**，不能按位置归属到某张
  （Rust 整段 `write_fmt` 持 stderr 锁，所以只是错位，不会撕行）。补偿=`eval` 在
  `--jobs > 1` 时把 `produce_recipe` 第三个返回值（typed `rationale::Note`）渲染
  进**本张**的块——同一条披露本来就在这条通道上。

  **④ `eval` 聚合表与完成顺序无关**。每张产出 `PhotoStats`（自己的
  `BTreeMap<metric, Acc>` + 曲线/RGB/蒙版分量），主线程**按 index 折叠**
  （`Acc::merge`）——`f64` 加法不满足结合律，按完成顺序求和会让 gap score 在末位
  ulp 上漂。旧的三个出口全部保形：分析失败仍打 FAILED 并继续；sidecar 读不出仍是
  硬错（走 `PhotoOutcome::Aborted` 带回**同一句** `read user xmp for <path>`），
  且置 abort 位让后续张次**不出队、不计费**——`--jobs 1` 下就是旧的立即停。

  **⑤ 计划外根修：`style::embed_preview` 暂存名撞车**。原名
  `autoshop-embed-<pid>-<tag>`，而 `pipeline::produce_recipe` 对每张都传常量
  tag `"query"` —— 一进程内两张并发（**R26 起就是三 worker 的 `batch`**、web
  的请求线程）就写同一个 `.png`/`.json`：一个 worker 嵌到另一张的画面（**拿到别人
  的风格向量**），或者互删对方的文件。修在 `embed_preview` 内部加进程内原子序号
  （`StyleIndex::build` 那个 `idx-{i}` 本来就唯一，未受影响）——一处修，所有调用
  点都好。

  **未验证/登记**：并行下的 API 侧节流**只有「N 个 propose 在飞」这一层**，中转站
  端点的 RPM 未知，没加全局限速；`claude` 子进程的并发内存不在预算常数内；
  ~~`eval --jobs > 1` 的**真实**跑批由主循环执行（本批零付费调用）~~ **← 已执行
  （2026-08-19，主审）：`--jobs 3` 全量 147 张实跑三次，第三次双流零回退落地
  为新基线（见 M-C 行）——顺带实证了时序器（转录全程按 index 序）、内存闸
  （3 worker 无 commit 事故）与注记归属（第二次尝试的 2 条回退披露精确挂在
  各自照片块内）**。**`serve` 与 GUI
  不共用这两个循环**（`batch_cmd`/`process_one`/`eval::run` 仅 CLI 调用，已 grep
  确认），行为零变化。

  门：clippy 双配置 0；测试 **695 lib（8 ignored）+ 10 CLI + 131 GUI + 2 + 2 合约**
  （lib +6 = `jobs` 的 sequencer/内存闸/pool/串行四组；CLI +1 = `--jobs` 默认值）。

- **AI 蒙版重算 + SigLIP 2 风格嵌入（2026-08-19，R27 Batch-5，~~未发版~~ →已随 v0.33.0 发布）** ——
  两件事，共用一条「本机 ML sidecar」纪律（设计稿：`~/.claude/plans/r27-materials/
  D1-ml-sidecar-design.md`，用户已批）。

  **T1 · 风格索引升级到 SigLIP 2（V2_PLAN §7-8 收口）**。新 sidecar
  `python/embed.py` + 桥 `src/embed.rs` + 配置 `AUTOSHOP_EMBED_SCRIPT`。
  模型 **`google/siglip2-base-patch16-384`**（768 维，HF 许可证标签
  `apache-2.0`，revision `f775b65a79762255128c981547af89addcfe0f88`，
  `model.safetensors` **1,501,968,264 B** / sha256 `ed72c0ac…`，实测下载
  **59.8 s**）。**选它是为许可证不是为跑分**：`openai/clip-vit-large-patch14`
  在 HF 上**根本没有许可证标签**，且卡片写「Any deployed use case of the model
  - whether commercial or not - is currently out of scope」，LAION H/14 卡片
  照抄同一句；本仓库是 PUBLIC 且正在做版权登记，而 SigLIP 2 既是无歧义
  Apache-2.0 又在公开基准上更强——这种「合规选项同时是更好选项」的情况必须拿下。
  推理栈**既不是 candle 也不是 ort**：本机 `onnxruntime` 的 provider 列表里
  **没有 CUDA**，ort 会退化成纯 CPU；而 torch 2.8.0+cu128 + transformers 5.2.0
  本来就在，sidecar 还能**直接 import `denoise.py` 的 `_fetch_verified`**——
  全树只有一份「下载并按 sha256 拒收」的实现，不是复制粘贴。
  **Rust 侧是「并排」不是「融合」**：`StyleExemplar::embed: Option<Vec<f32>>`，
  距离 = `Σ WEIGHTS[i]·(qᵢ−eᵢ)² + W_EMB·(1 − cos)` 两块相加，**绝不拼接**——
  所以 `W_EMB=0`、查询无向量、样本无向量三种情况都**逐位复现旧排序**，一份索引
  里混着有向量/无向量的样本也是合法的（`retrieve_with_embed`）。
  `CURRENT_INDEX_VERSION` **4 → 5**（v5 = 「可带 SigLIP 2 嵌入块，距离可带余弦
  项」），且 **v4 仍读**：嵌入是纯增量，拒收 v4 只会让老用户的 Style 面板白死
  一个小时的重建（R19）。**两个容量常量重新联立推导（R18 关闭）**：
  实测一条 768 维向量的 JSON 文本 **9,529 B ⇒ 12.41 B/元素**，最坏
  16 B/元素 ⇒ 12 KiB/条，加其余字段封顶 `MAX_EXEMPLAR_BYTES = 16 KiB`；
  `MAX_STYLE_EXEMPLARS` **50,000 → 5,000**、`MAX_STYLE_INDEX_BYTES`
  **32 MiB → 96 MiB**，5,000 × 16 KiB = 80 MiB ≤ 96 MiB，并用两条
  **模块级 `const _: () = assert!()`** 钉死——是**编译门**不是测试门，改任一个
  常量而不改另一个直接 `cargo check` 失败。**已登记的代价**：5,000–32,000 张的
  库现在撞样本数上限（以前撞字节上限），本项目实测过的最大库是 751 张，尚有 6.7×。
  **冒烟实测**（3 张 CC0 zoo 预览，经我们自己的 `autoshop decode` 抽出）：
  维度 768、L2 范数 1.0000000、**两次 GPU 单图运行 JSON 字节完全相同**（确定性）、
  CUDA↔CPU 逐元素最大差 3.2e-7（跨设备逐位相等**从未声称**）、三张互相余弦
  0.4643 / 0.5447 / 0.5917（有区分度）。**⚠ 与 D1 §5.2 的预期不符并登记**：
  batch=3 与单图跑同一张，最大差 **1.64e-7**——batch 尺寸**确实**会改末位（cuBLAS
  分块），所以「batch 不影响结果」应读作「不影响到 1e-7 以内」，`parse_vector`
  的 1e-3 范数闸覆盖它。**未标定**：`W_EMB = 2.0` 是量纲推理（14 维权重和 14.5，
  余弦距离跨度 0..2）**不是测量**，`AUTOSHOP_STYLE_EMBED_WEIGHT` 可改、0 复原旧
  行为；整条嵌入路径由 `AUTOSHOP_STYLE_EMBED` **默认关**（首跑 1.5 GB 下载 +
  每次调用重载权重）。

  **T2 · `Mask/Image` 臂落地（V2_PLAN §7-14，L-08 Arm C）——剩余拒收的主项**。
  新几何 `MaskGeometry::AiMask`（Batch-4 的 `Brush` 是先例）。**这不是导入，是
  重算，全篇纪律都建在这句话上**：sidecar 里**没有栅格也没有几何**（本轮把
  F2 的 105 例扩测到**全库 218 例**：21 个属性名、`MaskActive` 218/218 为 true、
  `MaskVersion` 218/218 为 1、`MaskSubType` 恰好 3 个值、唯一子元素
  `crs:Gesture` 83 例），所以只能**跑我们自己的分割器出我们自己的 alpha**。
  * **携带**：`subtype` / `ref_x,ref_y` / `name` / `mask_version` / 组合三元组，
    加一条 **白名单** `provenance: Vec<(String,String)>`（11 个从不解释的属性：
    三份摘要 + 版本标签 + `WholeImageArea` / `FullMaskSize` / `Origin` /
    `ModelVersion` / `ErrorReason` / `MaskSubCategoryID`）。白名单**两头都查**
    ——解析器遇到没量过的属性名直接拒（roundness 纪律），写回时再查一次
    （手改的 `recipe.json` 不能往 XML 里塞新属性）。`crs:Gesture` 的
    `Mask/Paint` 走**同一个** `parse_paint_stroke` 收进 `gesture`。
  * **写回**：`ai_mask_xml` 原样重发 LR 写的那个组件（自闭合/带 Gesture 两种形
    态都对），**SyncID 按本写手规矩重铸**，摘要原样带出去（它描述的是意图不是
    我们的栅格，重铸摘要等于伪造溯源）。
  * **渲染 = develop 时惰性重算**：`segment::resolve_ai_masks` 按
    `MaskSubType` 分派——1 → subject、2 → sky、**0 → 新的 SAM 2.1 Hiera-Large
    在 `crs:ReferencePoint` 上点提示**（Apache-2.0；repo `facebook/sam2.1-hiera-large`
    revision `665f8e2a…`，`model.safetensors` **897,897,416 B** / sha256
    `dc407dce…`，逐文件 sha256 + 字节闸后 `local_files_only=True` 载入——比
    subject/sky 两条老路**更严**，那两条只钉 revision，差距已在各自注释里登记）。
    **不装 torchvision**：transformers 5.2.0 把 `Sam2Processor` 解析成
    `Sam2ImageProcessorFast`，它硬要 torchvision；所以预处理**照 pinned
    `preprocessor_config.json` 手写**（1024² 方形压扁、resample 2 = BILINEAR、
    ImageNet 均值方差）并在运行时**回头断言配置没变**。输出取 logits→sigmoid
    的**软 alpha**（不是 `binarize=True` 的硬 0/1），`multimask_output` 三选一
    用 argmax + 显式最低下标破平（`torch.argmax` 的平局行为无契约），IoU 低于
    `--min-iou` **exit 3 拒收**而不是写一团坏 mask。
  * **缓存**：raster 落在照片自己的 develop 目录，键 =
    `(photo_key, subtype, ref 点, 后端代数)` 的 FNV-1a（**不是** `DefaultHasher`
    ——std 明说跨版本不保证，缓存名一升级就变，等于每次升级重跑模型）。
    `raster` 路径并入 `LocalAdjustment::bitmap_paths_mut`，于是 store 的
    相对化/解析/脱钩/快照/删版本清扫**一条走廊全覆盖**。
  * **披露强制且双向**：`MaskImportReason::AiMaskRecomputed` /
    `MaskLossReason::AiMaskRecomputed`，中文「AI 蒙版（本机重算，非 Adobe
    原栅格）」；另有**独立的** `AiMaskUnresolved`（「已带走，尚未重算」）——
    「近似了」和「根本没画」不是同一条消息，合并会让一次失败的模型运行读起来
    像一次成功的近似。画布徽标、蒙版行、导入行、导出行四处同步。
  * **失败绝不静默归零**：sidecar 缺失 / 依赖缺失 / 模型拒收 / 栅格不可解码，
    一律 `raster = None`，而 `render::is_raster_backed` 让这种蒙版**整条
    adjustment 被跳过**——不是按权重 0 渲染，因为 0 配上 `inverted` 会把这次
    编辑铺满整帧。原因逐条带出（`AiMaskResolution::unresolved`）。
  * **`coord_era` 迁移**：转参考点、**丢缓存 alpha**（旧帧里分出来的 alpha 不能
    转，只能重算），所以它**不属于** `recipe_has_raster_masks`（那句披露说的是
    「转不动」，而这个转得动）。

  **M-B 夹具前后实测（同一条测试、同一份夹具，HEAD `b39044f` worktree 对照）**：
  `_DSC8904` 13 条 correction **8 → 13** 导入（拒收 5 → 0）、`_DSC9082`
  **3 → 5**（拒 2 → 0）、`_DSC9414` **3 → 5**（拒 2 → 0），其余四份本来就 4/4、
  4/4、3/3、8/8 不变；**合计 33 → 42 个 mask，9 条拒收 → 0**。
  **端到端实跑**（Canon EOS R6 CR3 + 一份带 `ai_mask` 的 recipe，`autoshop apply`）：
  冷跑 **48.0 s**（含模型加载 + 推理）写出 `ai-mask-8648066c82c17d67.png`
  **4096×2731 8-bit 灰度 715,381 B 覆盖率 11.90 %**（长边帽生效），热跑
  **9.8 s** 并打印「1 reused from the develop cache」——模型没有再跑。

  **门**：clippy ×2 = 0；`cargo test` + `--bins --features gui` 全绿
  （lib **669 → 695**，其中 688 pass + 7 条既有 ignored 探针；CLI 9 / GUI 131 /
  合约 2+2 未动）；`AUTOSHOP_LR_PROBE_FIXTURES` **16/16 字节往返**；
  `AUTOSHOP_MB_FIXTURES` 全绿（计数见上）；`AUTOSHOP_RAW_ZOO` **9/9**；
  `audit_i18n` 全 9 门 0（新增 5 条 zh 词条）；**字体 cmap 门绿且无需重新生成
  子集**——新增汉字「本/机/重/算/非/原/栅/格/尚/由」逐字验证均已在
  `assets/fonts/NotoSansSC-autoshop.ttf`（739 字）里，`▨`/`×`/`—` 在
  symbols2 里。**10 条变异逐条验证红→回滚**（转录在 batch5 报告）。
  新增 tier-3 环境门 **`AUTOSHOP_SEG_PROBE`**（未设静默、设了跑真 SAM 2.1：
  实测 Canon 预览 1536×1024 覆盖率 0.1272、soft=true）。

  **本轮不做（登记）**：① Object 与 **Background** 在 sidecar 里**不可分**
  （都是 `MaskSubType=0`，`MaskSubCategoryID` 全库 218 例只有 8 例、2 个值，
  远不够立枚举）——一律按「点下去的那个物体」分割并披露，不猜。
  ② 我们的 alpha **离 Adobe 有多远没测**：sidecar 没有栅格，不做一次受控 LR
  导出就无从比较（V2_PLAN §7-14 的剩余收口）。③ subject/sky 两条老后端仍只钉
  revision 未钉字节（D1 §3.3 的 sha256 逐文件闸只上到了 SAM 这条新路）。
  ④ BiRefNet 未接（要 torchvision，D1 §3.2）——subject 仍是 rembg/U²-Net。
  **← 主审裁决（2026-08-19）**：接受但**不定案**——subject 臂三案（u2net 现状 /
  BiRefNet+torchvision / SAM 按 ReferencePoint 点提示，后者已集成零新依赖）的
  质量差异**均无实测**（D1 §9 #7/#8 本就未测），按「蒙版不许猜」律押到发布
  **目检包**：用户对真实照片比对三案蒙版后再裁升级与否。
  ⑤ D1 §9 的 8 GB 卡运行时/显存估计**仍是估计**，本轮只实测了 SigLIP 2 与
  SAM 2.1 的端到端墙钟，没有单独测显存峰值。⑥ GUI 里没有「现在重算这条 AI
  蒙版」的按钮——重算跟着 develop 走。

- **画笔蒙版结构落地 + 分割许可证根修（2026-08-19，R27 Batch-4，~~未发版~~ →已随 v0.33.0 发布）** ——
  L-08 由「登记不修」转正为**根实现**（用户裁决：最高质量、笔刷必须无损往返、
  不许位图降级）。**一句话**：`Mask/Aggregate` + `Mask/Paint` 从此是一等几何，
  **读进来、存进 recipe.json、原样写回 sidecar**；唯独**不渲染**——因为
  sidecar 存的是**笔画**，从来不存 **alpha**，落笔核（硬度→衰减 + 逐 dab 累积律）
  只能靠一次受控 LR 实验测出来，而那次实验尚在途中。这是**结构，不是渲染**。

  **① 新几何 `MaskGeometry::Brush`（recipe.rs）**。按 F2 实测编码逐字承载：组级
  `(MaskBlendMode, MaskValue, MaskInverted)` + `MaskName`，子级每条 `BrushStroke`
  带 `MaskValue`/`Radius`/`Flow`/`CenterWeight`/`MaskSyncID` 与 **`crs:Dabs` 令牌流
  （逐字符串携带，换行分隔）**。令牌语法 `r <f>` / `f <f>` / `h <f>` / `d <x> <y>`
  四型（22,966 token / 382 组件，零畸形）写在类型文档里但**本轮不求值**。
  **为什么是字符串而不是 `Vec<Dab>`**：唯一缺的是 alpha 核，先给流套一个结构等于
  围着一个还没人写的渲染器把形状冻死；逐字承载则保证这期间往返精确。schema 变更
  搭 v0.33 已有的破坏性变更（`deny_unknown_fields` 早已拒未来字段）。
  **② 导入解锁**。带 Paint/Aggregate 的 correction 整条进来了——不再连同**旁边
  那些引擎本来就画得出的参数化形状**一起丢掉（这正是 L-08 登记文本自己的抱怨：
  全库 18 条 correction + 14 个可绘形状因邻居是笔刷而被扔）。有参数化形状时它作
  base、笔刷组作 `MaskComponent`；纯笔刷 correction 则第一个组当 base。
  **③ 写回**。`brush_mask_xml` 按实测形状重发 `Mask/Aggregate` + `Mask/Paint` +
  `crs:Dabs`；数字走 `f32` 的 `Display`（最短往返表示），**不过 `local_fmt`/
  `lr_num`**——这些值是从同一份 sidecar 抄进来的、从未参与运算，四舍五入等于替
  摄影师改了他没改的数。`crs:MaskSyncID` 按 writer 既有规则重新铸造（**已接受的
  代价**：Lightroom 给某条笔画的 ID 只活过一次保存，描述**笔画本身**的一切则每次
  保存都活着）。
  **④ 两向具名披露**。`MaskImportReason::BrushCarried` /
  `MaskLossReason::BrushCarried`，英文一致为「brush mask(s) carried, not yet
  rendered - kernel measurement in flight」。导出侧这一条是这个枚举里**唯一
  反向**的成员：投影是完整的，**渲染器**才是缺的，所以它的中文没有套在
  「不会带走」那句框架里。`render::mask_weight` 对 Brush 恒返 0——在 Add
  (`1−(1−w)(1−0)=w`) 与 Subtract (`w·(1−0)=w`) 下**都惰性**，旁边的渐变照画不误。
  **⑤ 三处解析隐患一次修完（F2 §5.1，本轮之前都是「还没被踩到」）**。
  `classify_correction` 原来**平铺**走蒙版块 → 组内 Paint 被当成组的**兄弟**；
  `base_geometry_at` 原来扫**整段 correction**、不辨嵌套 → **嵌套 Look 里那条
  `Mask/Gradient` 会被选成 correction 的 base**（变异实测：导入成
  `Linear{0.9,0.9→0.1,0.1}`，摄影师从没画过的形状）；`parse_one_correction` 的
  几何键读的是**一路读到 correction 结尾**的切片 → 后面组件的 `MaskValue` /
  `MaskInverted` 可以替省略了该属性的 base 回答。三者统一走新的 `components_in`
  深度遍历 + `base_element` 元素定界，各配回归测试。
  **⑥ 不猜的边界**：Aggregate 子级非 Paint、嵌套深度 >2、空组、Paint 自带
  `MaskBlendMode≠0` 或 `MaskInverted=true`、缺九属性之一、`{r,d,f,h}` 之外的
  token、无 `crs:Dabs`——171 份真实 sidecar 里**零反例**，一律 `OutOfModel` 拒收
  并具名，绝不猜（roundness 那条纪律用在笔画上）。
  **⑦ `Mask/Image` 仍拒**（全库 105 个、78 条 correction、40 个文件）。它**没有
  栅格载荷也没有几何载荷**（全库最长属性值 55 字符），只有意图
  （`MaskSubType`/`MaskName`/`ReferencePoint`）、溯源摘要与代理帧尺寸。要复现得
  自己跑分割器出自己的 alpha——那是**另一个功能**（近似再导出、并如实披露），
  不是解析器够得着的东西。**这是剩余拒收的主项，任何笔刷工作都碰不到它。**

  **⑧ 分割 sidecar 许可证根修（D1；PUBLIC repo + 版权登记在即）**。
  `python/segment.py:65` 的 `nvidia/segformer-b0-finetuned-ade-512-512`，其权重
  许可证「NVIDIA Source Code License for SegFormer」原文：*"The Work and any
  derivative works thereof only may be used or intended for use
  non-commercially. Notwithstanding the foregoing, NVIDIA and its affiliates may
  use the Work and any derivative works commercially. As used herein,
  'non-commercially' means for research or evaluation purposes only."*
  （本轮直连 `raw.githubusercontent.com/NVlabs/SegFormer/master/LICENSE` 复核原文）
  ——它限制的是**使用**，不是分发，所以「我们只下载不分发」治不了。换成
  `shi-labs/oneformer_ade20k_swin_large`（HF `cardData.license == "mit"`、非
  gated，本轮直连 HF API 复核），**且是更强的模型**：ADE20K mIoU 57.0 单尺度
  对 SegFormer-B5 的约 51，更远高于实际在跑的 B0——**许可证干净的那个同时是更好的
  那个，没有质量取舍**。第二处 `segment.py:45` 的裸 `remove(img, only_mask=True)`
  会解析到**该次安装的默认模型**：2.0.76 上是 `u2net`（Apache-2.0）没问题，但
  上游已把默认换成 `bria-rmbg`（商用需付费协议）——一次 `pip install -U rembg`
  就能在源码零改动的情况下把付费模型换进公开产品。改成显式
  `new_session("u2net")` 钉死。
  **实现要点**：OneFormer 是**掩码分类**模型，没有逐像素类 logits 可 softmax；
  transformers 自己的 `post_process_semantic_segmentation` 求
  `seg = Σ_q softmax(class)·sigmoid(mask)` 后**取 argmax**（把软 alpha 扔了）。
  我们要 alpha 且只要 150 类中的 1 类，所以只对 sky 那一列做 query 轴收缩
  （`einsum("bq,bqhw->bhw")`）——**算术与上式完全一致**，且从不物化 150 平面。
  原来「低分辨率 softmax + 单平面上采样」的省内存推理**原样保留**（61MP 全平面
  上采样是 ~36GB float32）。`id2label` 查表仍**动态**，但改成**先精确匹配**：
  ADE20K 同时有 `2=sky` 与 `48=skyscraper`，旧的子串匹配两个都中、再靠 dict 插入
  序取第一个——摩天楼不是天空。另加确定性开关（cudnn.benchmark off /
  deterministic on / TF32 off：这份 mask 是被 recipe 引用的**文件**，R21 的版本指纹
  要比它的字节）与 GPU 自动选择（Swin-L 是原 B0 的约 60 倍参数，纯 CPU 会把约 1s
  变成一分钟）。**登记不修**：`from_pretrained(revision=...)` 只钉**哪棵树**，不
  证明**字节**；denoise.py 那种 sha256 逐文件闸需要自己取文件再本地目录加载
  （D1 §3.3），比一次许可证修复大，明写在 `SKY_MODEL` 注释里而不是留给以后被发现。

  **门**：clippy ×2 = 0；`cargo test` + `--bins --features gui` 全绿（662+9+131+2+2）；
  `AUTOSHOP_LR_PROBE_FIXTURES` 16/16 字节往返；`AUTOSHOP_MB_FIXTURES` 全绿；
  `AUTOSHOP_RAW_ZOO` 9/9；`audit_i18n` 0；字体 cmap 门绿（新增 zh 词条逐字过成品
  子集）。**6 条变异**逐条验证红→回滚。
  **M-B 计数未动，且原因可测**：那 7 份 sidecar 里带 Aggregate 的 3 条 correction
  **同时**带 `Mask/Image`，所以仍拒——33/42 前后一致。真正的改善在有纯笔刷素材的
  目录上：`_DSC9583` **8/11 → 10/11**（Mask 1 与 Mask 7 进来了，各带
  `BrushCarried`）。Mask 2 仍拒但**换了正确的理由**：`CorrectionAmount="1.1"`
  超出 `LocalAdjustment::amount` 的 0..1，`OutOfModel`——从前它被含混地报成
  「AI / brush correction(s) skipped」。
  **待办**：alpha 核实验（`~/.claude/plans/lr-brush-kernel-*.md`）落地后才谈渲染；
  `Mask/Image` 需自有分割器；brush 的 dab 坐标不参与 `coord_era` 迁移（与位图同列，
  `recipe_has_raster_masks` 一起报「转不了」）；`crs:MaskSyncID` 单次存活已接受。

- **台账勘误 + 登记转正（2026-08-19，R27 Batch-1a，~~未发版~~ →已随 v0.33.0 发布）** —— 不是一次发布，
  是一次**审计留痕**：R27 立案时对全部文档做了穷尽扫描（`A-leftover-ledger.md`
  33 项），查出台账多处滞后于事实，以及若干「登记不修」在本轮被用户裁决为「修」。
  逐条列在这里，便于日后追溯**是谁、在哪一天、依据什么改的**。下面被更正的原句
  一律保留（删除线 + 箭头），不做静默重写。
  **① 素材台账（M-B / 六合一实验）**：`M-B` 行的「Defringe 非零仍欠」与
  v0.31.0 条的「素材欠（用户侧一次 6 合 1 LR 实验即可全取）」六项、v0.30.0 条的
  「Defringe 全库 0 待用户 LR 补做」——**全部已于 2026-08-18 交付**，逐项证据：
  Defringe 非零 `_DSC9597.xmp: DefringePurpleAmount="10"` / 径向旋 +30°
  `_DSC9600.xmp: Angle="28.229232"` / 蒙版 Hue +50 `_DSC9594.xmp:
  LocalHue="0.277778"` / 蒙版 Sharpness `_DSC9594.xmp: LocalSharpness="0.803738"` /
  Subtract 复制 `_DSC9598.xmp: MaskBlendMode="1"` / Auto CA `_DSC9596.xmp:
  AutoLateralCA="1"`；另有两份附赠 `_DSC9595.xmp: Sharpness="150"`、
  `_DSC9593.xmp: LocalExposure2012="1"`。
  **② L-14 Defringe 乙案**：标签定义表 `B3` 行早已写明「Defringe 走甲案＝
  CarriedOnly 终态」，且 SF4 已由用户拍板 = C；v0.31.0 条把「Defringe 乙案（渲染
  近似）」列为「等同一次 LR 实验」是**滞后行**。**裁决：甲案终态成立，乙案不做**。
  **③ 90° 旋转归属**：v0.31.0 条写「v0.32 首项」，v0.32.0 条写「仍留 v0.33」，
  两行相冲。**以 v0.33 为准**（v0.32.0 那轮被径向几何根修占满，功能顺延一版）。
  **④ L-16b / L-16d 两条「登记不修」自陈不可达**：崩溃窗孤儿 meta 行、web「RAW
  不可读仍回存」窄残留——文本本身就说「已滤」「实际不可达」，登记为**已关闭的纵深
  防御注记**，不再算未决项。
  **⑤ 本轮用户裁决（2026-08-19）**：L-16a Analyze 继承 `refuse_unresolved_strip`
  =**终态**；L-26 macOS/Linux 字体回退表留空 = **保留开口**（**不**宣布
  Windows-only，等真机字体路径实测）；L-19 两项既裁成本（App 内删光蒙版不传播到
  sidecar、往返把 `crs:Flipped` 并入反相位）= **定案**；L-09 AI 精修重置
  `midpoint`/`mask_version` = **修**（已实现，见⑥）；V2_PLAN §7-8 CLIP 升级 =
  **批准**，从「决策项」转为 R27 工作项。
  **⑥ 登记不修 → 已修（代码面，本批落地，随 R27 发版）**：L-07 CLI 蒙版导入损失
  披露（`main::lightroom_import_note`，`analyze`/`auto`/`match` 三处随 `xmp ->`
  行发 stderr；`batch` 不需要——它的待办集按 `has_develop_or_sidecar` 过滤，带
  sidecar 的照片根本不进）；L-09 见⑤；L-16c `straighten≠0` 时的裁剪迁移（详见⑦）；
  L-17 第三类前向静默 → **响亮拒收**（`MaskGeometry` 与 `RangeMask` 两个内部标签
  枚举补 `deny_unknown_fields`——「serde 不支持」这句旧断言经实测证伪：加在
  **容器**上即可，标签字段本身自动豁免。**前向后果**：从 v0.33 起，旧 exe 遇到新
  版写的蒙版几何会**按字段名点名拒收整个 recipe**，不再静默丢字段再写回。这与
  `EditRecipe` 顶层的既定姿态一致，也是「不支持降级」的成文依据）；L-25
  `explorer.exe` 逗号路径（explorer 自行按**逗号**切命令行——它的开关本就写作
  `/select,<path>`——故改为 `raw_arg` 传加引号的整条路径；路径含 `"` 或非
  Unicode 时不建原始命令行，回落旧写法）；L-29 见上方「发版链 + 环境门套件」。
  **⑦ L-16c 判定（读码定性，非猜测）**：R24 登记的「`straighten≠0` 时 crop 迁移
  一阶近似」**只有一半是真的**。裁剪矩形归一化的参照系是**拉直后的内接帧**
  （`render_pipeline` 先 `rotate_straighten` 再 `apply_crop`），而
  `inscribed_dims` 对 `w↔h` **严格交换等变**（一般支路一望即知；细支路里的
  `if w >= h` 看似破坏对称，实则在 `w == h` 上**不可达**——该支路要求
  `sin 2a ≥ 1` 即恰 45°，那里 `s == c` 两个输出本就相等）。加上旋转互相可交换，
  **四个纯旋转态的裁剪迁移本来就是精确的**。真正的残差是**符号**：
  `rot(deg)∘镜像 == 镜像∘rot(−deg)`，所以四个**镜像**态下拉直角被反着用了
  `2·deg`。修＝对镜像态取反 `straighten_deg`（与椭圆 `angle` 同一条规则）。
  八态至此全精确。新测试两条 + 变异验证在案。
  **⑧ V2_PLAN / M1_PLAN 同步**：§7-2 正端点关闭（`_DSC9593`）、§7-3 折入三组
  实测对、§7-10 `LocalSharpness` **当日走完一个完整弧线后以 ÷100 定案**——用户
  先确认是**手输** 80（手输应写 `0.800000`，文件是 `0.803738`，一度判「÷100 被
  证伪」），随后 R27 网证（F3）拿到两个不依赖该文件的 Adobe 侧锚点：Adobe 自家
  Soften Skin 本地预设 **+25 → `0.25`**、MIDI2LR 全滑杆拉满 **+100 → `1`**；
  手输假说所需除数 99.535 会把 ±100 端点推到 ±1.0047（Adobe 恰坐在 1.0 上），
  且公共语料 18 个非零值里 10 个任意实数、无量化网格 ⇒ **÷100 成立，0.803738
  =滑杆实际停在 80.37，让步的是回忆**（用户 2026-08-19 接受定案；证据档案
  `~/.claude/plans/r27-materials/F3-web-evidence/`）、§7-8 CLIP 升级
  批准；M1_PLAN §8 表退役一遍（1/2/4/7/8/10/11/13/14 关，3/6/9/12 留开并各自换
  新计划）、§9.5 重写。
  **⑨ 过时注释更正（纯注释，零行为）**：`xmp.rs` `MaskImportReason::Rotation`
  与 `a_lightroom_radial_with_angle_imports`、`bin/gui/tests.rs`
  `the_rotation_warning_says_why` 三处仍写「`crs:Angle` 符号/枢轴未验证」——
  v0.32.0 已实测三项，现存唯一情形是「文档没声明画幅」；`render.rs`
  `radial_roundness_is_a_documented_no_op` 写「尺度与符号未验证」——v0.31.1 已
  测定域为 ±100 整数滑杆，未测的只剩**像素含义**。
  **⑩ 本批门（未发版，仅记数）**：624 lib（+4）/ 9 CLI（+1）/ 130 GUI（+1）/
  2+2 合约，`cargo clippy --all-targets` 与 `--all-targets --features gui` 双 0，
  无新 i18n 词条（CLI 披露走既有英文 stderr 通道），六条新测试全做变异验证。
  **⑪ V2_PLAN §7-9 OpenAI Images 六子项当日付费实测闭**（用户批准，3 次 edits
  调用 <$0.3，原始响应存 `~/.claude/plans/r27-materials/probe-images/`）：
  `gpt-image-1.5` 存在；`chatgpt-image-latest` **存在＝原「absence」断言证伪**；
  `input_fidelity` 在 **gpt-image-2 上 400 拒收**（生产配置严禁下发该参数）、在
  1.5 上 low/high 皆 200、机制＝输入图 token 深度（194 vs 4354）；掩码透明区=
  编辑区、**不透明区仅近似保留**（RMS 5.6/255，整帧重绘）⇒ object-removal 集成
  必须自行逐位回贴非编辑区；`b64_json` 为默认响应、`usage` 图文分列且 1.5 计
  输出 text token；docs 主机对程序化抓取 403＝不可依赖。
  **⑫ L-27 think-envelope 字段序当日付费实测闭**（一次真实调用，探针按设计
  响亮失败）：strict 结构化输出**不按声明序生成**——响应键严格字母序
  （`recipe` 偏移 170 先于 `scene`=1269/`tool_plan`=1595），「先想后写」只剩
  prompt 约束；信封的结构收益（显式计划/可审自评/逐族取舍）不依赖生成序、
  不受影响。机制假说（n=1，恰为字母序）＝端点按键名排序序列化；**登记可选项
  不即取**：改思考字段名使其字母序先于 `recipe` 可借该机制强制顺序，动名前
  须二次探针证机制（探针留 `#[ignore]`+env 门，重跑即机制检）。
  `catalogue.rs` 注释同步更正。M1_PLAN §8 #3（结构化输出 schema 上限文档锚）
  仍开，等下次带真机付费会话顺跑。

- **通用输入格式（2026-08-19，R27 Batch-1b，~~未发版~~ →已随 v0.33.0 发布）** —— 项目自立项起**只
  跑过 Sony ARW**（`ARCHITECTURE.md` 旧行「resolved: Sony `.ARW`」为证）。本批
  把 RAW 从 9 扩展名扩到 **24**，把 baked 从 5 扩到 **8**，并**第一次**用 9 份
  CC0 真机样张（raw.pixls.us，sha256 逐份核对，存仓库外
  `~/autoshop-fixtures/raw-zoo/`）跑通 Canon CR2+CR3 / Nikon NEF / Fuji RAF /
  Olympus ORF / Panasonic RW2 / Pentax PEF / Ricoh DNG / Sony ARW ~~九个厂牌~~
  （←2026-08-20 勘误：实为**八厂牌九机**，Canon 占 CR2+CR3 两台；`decode.rs`
  zoo 注释同日同改）。
  **① 一份清单，处处派生**：`decode::RAW_EXTS` 与 `pipeline::BAKED_EXTS` 是全
  仓库仅有的两份扩展名表。原先有**四份手抄副本**且 web 那份已漂移（`.orf`
  `.rw2` `.raw` 四个版本里一直进不了网页选择框）；现 GUI 对话框 / serve 上传门
  / 库扫描全部派生，唯一不可派生的 `index.html` `accept` 由测试钉死并在失败时
  直接打印可粘贴的替换串。**② `.x3f` 永久排除**（rawler 0.7.2 的 `raw_metadata`
  是字面 `todo!()`）。**③ ORF 类降级**：rawler 对 24 个格式中的 12 个不实现任何
  内嵌预览方法，旧码把「格式本就不存预览」和「预览损坏」混成同一个错误，导致
  `.orf` 在 CLI 上直接失败而同一文件在 GUI 里能开 —— 现降级为自家中性 develop
  并**明说**。**④ CLI 补 `catch_unwind`**（GUI 自 v0.22 就有；第三方解析器一次
  panic 旧行为是整个 `batch` 进程死）。**⑤ 拒绝分三种并各带 DNG 转换出路**
  （未知厂牌 / 未知机型 / 无解码器；`decode::DNG_ONRAMP` 单一出处）。
  **⑥ 单色与四色传感器改为 develop 之前拒收**（旧码跑完整条 demosaic 才 bail）。
  **⑦ baked 真 EXIF**（ISO/快门/光圈/焦距/日期/厂牌/机型），走 rawler 自带
  `Exif::new` + `GenericTiffReader`（JPEG 需先走 APP1 段），**零新依赖**，且与
  RAW 臂**共用同一份提取**（`exif_facts`）。**⑧ 新增 webp/bmp/gif**（三者均纯
  Rust、无 `build.rs`、无 `links =`，已逐一核 Cargo.lock；avif/heic 仍按
  `Cargo.toml:54-56` 的「无 C 依赖」红线排除）。
  **⑨ 三条实测新发现（读码猜不出来的）**：
  (a) **rawler 用 Bayer 算法 demosaic X-Trans**（`PPGDemosaic` 自述只能用于
  RGGB，而它的守卫 `CFA::is_rgb` 只检查 pattern **名字**里的字母、不看几何，
  X-Trans 6×6 直接放行；`ppg.rs:126-136` 的绿通道插值假定四邻皆绿+隔行同色，
  两条都只在 2×2 quincunx 成立）—— 原 B 报告列为「离线无法判定」的 R6 就此**判负**。
  处置＝**每次渲染披露**而非拒收（色/影调/构图都对，只有极细节近似）。
  (b) **v0.32.0 的 (32,20) 原点缺陷是 per-BODY，不是 per-make**：动物园里的
  Sony A7 III **不需要**该修正，因为它的 `SonyRawImageSize` 交出的是**整块**
  6048×4024，而 A7R IV 交出的是已裁的 9504×6336。九台机器**无一** fire，
  R2 风险就此关闭（也说明扩格式没有偷偷扩大该修正的作用域）。
  (c) **B 报告两处读码结论被实测更正**：`cr2.rs:284` 的
  `active_area = crop_area` 只在 `cpp == 3`（sRAW/mRAW）分支里；NEF/ORF 的
  `active_area` 并**非** `None`（相机库 borders）。tier-1 表已换成九组**实测**
  数字，不再是猜测。
  **⑩ 三处只披露不拒收**（连同 (a)）：DefaultCrop 越界（旧为静默 `None`，
  `render.rs:164` 直接丢弃返回值）、16-bit 无 ICC（按 sRGB 读——8-bit JPEG 这
  样几乎总对，16-bit 常错，故只对 16-bit 出声）、`.tif` 装 RAW 则**拒收**并点名
  依据（旧行为是 `image` crate 解出 DNG 第一个 IFD ＝**缩略图**，照片会「打开成功」
  却只有几百像素——开错比开不了更糟）。
  **⑪ `batch --include-baked`（默认关）**；`eval` / `style-index` **明确不做**：
  两者都靠兄弟 `.xmp` 判定「用户自己的编辑」，而 baked 的邻居 sidecar 按
  `store.rs:3106-3111` 既定政策「不是我们该解释的」，加进去等于从后门推翻该决定。
  **⑫ 本批门（未发版，仅记数）**：634 lib（+10）/ 9 CLI / 130 GUI / 2+2 合约，
  `cargo clippy --all-targets` 与 `--features gui` 双 0，`AUTOSHOP_LR_PROBE_FIXTURES`
  16 份 + `AUTOSHOP_MB_FIXTURES` 7 份仍绿，i18n audit exit 0（无新词条——新披露
  都走既有英文 stderr 通道）。新增 tier-3 环境门探针 `AUTOSHOP_RAW_ZOO`（未设
  静默、设了读不动就**响亮 panic**，同 `AUTOSHOP_MB_FIXTURES` 纪律），9/9 通过。
  五条变异验证有转录。
  **⑬ 本批发现的两处收尾**：CLI `decode` 把 `meta.width/height` 打成
  `sensor:`，但按 L05-6 那两个字段描述的是**交付的预览**（Pentax 打 640×424、
  DNG 打 160×120 即为此）——既有标签错误，**主审裁定＝修**（改标 `preview:`，
  与项目一贯的 WYSIWYG 错位根修同理）；新披露只走 stderr，GUI 无控制台看不到
  （与既有 `ValidatedRecipe::disclose`、embedded-XMP 警告同一现状，扩大到 GUI
  toast 通道是带 i18n 面的独立改动，**登记不做**）。

- **90° 手动旋转骨架（2026-08-19，R27 Batch-2，~~未发版~~ →已随 v0.33.0 发布）** —— v0.33 首项落地。
  骨架表（本文件下方 v0.31.0 段）已就地补「落地状态」栏并按删除线纪律更正三处
  滞后断言（消费点**五个**不是三个、缓存盐**两个**不是一个、`↶/↷` 字形**撞门**）。
  **① 根因洞见成立**：EXIF 八态就是正方形的二面体群、对复合封闭，所以用户的
  四分之一圈与相机方向能合成**同一个** `Orientation`，引擎**不新增第二个旋转
  阶段**——`render::compose_orientation` 是全部改动，`oriented`/`orient_point`/
  `orient_recipe_coords` 原样复用。
  **② 一个 mover**：`pipeline::rotate_recipe` 同时管几何（走 delta）、栅格蒙版
  （真转、新号、全有或全无）与转数本身；GUI 的画布靠 `sync_base_turns` 每帧
  按 `base_turns` 与 `recipe.quarter_turns` 的差把**底片像素**转过去——不给画布
  加旋转阶段，是因为 `canvas.rs` 里每一处屏幕↔画幅映射（`view_norm_to_orig`、
  裁剪把手、涂抹画布、覆盖层、修复选区）都以底片为准，转像素等于一次性把它们
  全转了。撤销/重做/切换变体/载入版本因此各自**不需要**旋转代码。
  **③ 三类前向后果**（`deny_unknown_fields` 既定姿态）：v0.32 exe 对**带转数**
  的 recipe 按字段名硬拒；**未旋转**的 recipe 与 v0.32 逐字节相同故照读不误
  （`skip_serializing_if` 换来的）；R21 结构指纹**无需**重归档。
  **④ 门**：642 lib（+8）/ 9 CLI / 131 GUI（+1）/ 2+2 合约（主审第一方复跑核
  实——本条初稿误记 646/+12/6 条），`cargo clippy --all-targets` 与
  `--features gui` 双 0，audit_i18n exit 0（新增
  7 对 en+zh 词条），`subset_gui_fonts.py --check` **803/803**（`⭯`/`⭮` 早在子集
  内，免再生成；四个汉字撞门改写），`AUTOSHOP_LR_PROBE_FIXTURES` 16 份仍绿，
  `AUTOSHOP_RAW_ZOO` 9/9 仍绿（并加了两条转数臂）。六条新测试全做变异验证。
  **⑤ 登记不做**：~~A7/A8（XMP `tiff:Orientation` + 画幅供给）等一份竖幅 LR
  导出~~ **← R27 Batch-3 已落地**（P1 的取证不需要新导出：用户图库里现成的三张
  `Orientation=8` 竖幅成片 + 四张横幅对照就把画幅判死了，7/7）；
  带烘焙像素的照片不可旋转（按钮禁用并说明理由）；web UI 缩略图不跟随转数。

- **裁剪几何 + 画幅根修（2026-08-19，R27 Batch-3，~~未发版~~ →已随 v0.33.0 发布）** —— 把 R27 的三份
  测量（P3 裁剪角度模型、P5 裁剪帧下的蒙版帧、P1 竖幅蒙版帧）一次性消费进代码。
  **一句话**：`crs:Crop{L,T,R,B}` 与 `Mask/CircularGradient` 的
  `Top/Left/Bottom/Right` 是**同一种编码**——旋转后的两个对角，写成**未旋转源
  画幅**的分数——本轮之前引擎两处都读错，且错法不同。

  **① `crs:CropAngle` 的符号（P3 §4，HIGH；最弱一张 34× 余量，最好一张 7257×）**。
  `rot(源→导出) = −CropAngle`：Lightroom 把**画面**逆时针转 `+CropAngle`，等价于
  把裁剪**框**顺时针转。引擎 `rotate_straighten` 正角是顺时针，所以
  `straighten_deg = −CropAngle`，**负号只落在 XMP 边界**（与 `k=1.032` 同一条
  纪律，引擎内不含它）。修前是 1:1 直取 → **每一份带拉直的导入都反了
  2×CropAngle**（图库最大一张 6.55°）。M1_PLAN §8 #9「the one that matters」
  由此关闭。
  **② 裁剪矩形读错帧（P3 §3.2；7/7 像素精确、零自由参数）**。四个数是旋转后
  对角：`X=(R−L)/2·W`、`Y=(B−T)/2·H`（**带符号**），`p=X cosθ+Y sinθ`、
  `q=−X sinθ+Y cosθ`，导出尺寸 `round(2p)×round(2q)`。**裁剪不带 `k` 放大**
  （全局最佳单尺度 1.000006，蒙版那边是 1.0315）——同一命名空间、同一编码、
  不同仿射，这条不对称已写进码内注释。七个候选全被证伪，其中**引擎旧读法
  （天真外接框）差 485 px、0/7**。落地构造＝**在 XMP 边界把 LR 的（角点，角度）
  折成引擎的（内接帧裁剪框，拉直角）**：`d = R(−θ)·(centre − c_src) + (W_i/2,H_i/2)`，
  渲染管线一行不动（P3 §6.2 的「保留现序」式）。**唯一不精确处已实测并逐张
  登记**：引擎的内接帧是**居中**的，LR 的框贴着旋转帧边缘时会越界，七张实测
  越界 0 / 0.216 / 1.243 / 1.251 / 6.909 / 30.260 / 5.895 px（最坏＝一条边的
  0.55 %），越界即**裁剪并披露**（新 `xmp::crop_import_note`，走 `store`/CLI
  已有的 stderr 通道）——不披露就会被 `EditRecipe::clamp` 在第一次保存时静默
  做掉。
  **③ 排序守卫删除（P3 §6.3）**。旧读侧要求 `left<right && top<bottom`，而
  `Left>Right` 在角点编码下**合法且可达**（2:3 框拉直 >33.69° 即出现，
  `CropAngle` 自身值域 ±45°）——旧码**静默丢掉整个裁剪**。`[0,1]` 那半守卫保留
  （两个点都是 LR 保持在画幅内的矩形角点）。
  **④ `CropAngle` 精度 `{:.1}`→`{:.6}`（P3 §6.4）**。导入 `_DSC9298` 再存回原本
  写成 `-3.3`：0.0256° 漂移＝9504 px 帧上 4.3 px 端到端倾斜（该精度最坏 8.3 px）。
  **⑤ 蒙版帧＝未裁剪的源帧（P5 §1，HIGH）**。`probe4/PROBE4-FINAL.md` §4 开头
  「`W,H` = 导出像素尺寸」那句在 `HasCrop=True` 时**是错的**，正确读法是
  `DefaultCropSize`：`DSC09401_16.9.JPG` 上按导出尺寸解会把五个径向位移
  **834–1384 px**。证据＝跨裁剪对里 23 个带数值几何的蒙版 **22 个字节相同**、
  匹配滤波把任何「裁剪帧耦合」压到 **0.09 %**，而同一台仪器上的阳性对照
  （PostCropVignette）读 **6.1 DN**。**本文件与 V2_PLAN 引用该句处已就地勘误**。
  **⑥ 竖幅＝未旋转传感器阵列（P1 §1，HIGH；7/7 文件靠 `dSS` 各自选中真帧，
  四张横幅对照复现已知答案）**。`tiff:Orientation=8` 的导出**已经是正的**、
  **不带**方向标签、连 `tiff:` 都没有，而它内部的 `crs:` 坐标仍是 9504×6336 的
  分数；一个纯径向声明 +1.6 EV，按传感器帧读回 **+1.647**，按显示帧读回
  **−1.982（符号都反）**。落地＝`FrameAspect` 现在带 `(W, H, turn)`，读侧解完源
  帧后**整份 recipe 过一次 `render::orient_recipe_coords`**（`coord_era` 迁移已有
  的代数：裁剪、蒙版、范围采样点、镜像下的拉直符号一次全搬），写侧过其逆。
  这也让导入 recipe 的 `coord_era` 戳**第一次是真的**（此前是给传感器帧数字贴
  显示帧标签）。P1 §4.2 登记的反证（同文件 Mask 3 反过来指向显示帧，0.5σ vs
  9.6σ）一并搬进本台账，不因为结论好看就丢。
  **⑦ A8：我们自己的文档现在声明画幅**。新写文档带
  `tiff:ImageWidth/ImageLength`（**源帧**——F3 公开普查 72/72：sidecar 声明的
  永远是传感器帧）+ `tiff:Orientation`（**合成后**方向，EXIF ∘ 用户转数，
  关掉 Batch-2 登记的「sidecar 描述未转帧」缺口）。合并路径**只在底稿完全没提
  `tiff:` 时**才补声明（提了就用底稿的；提了但不成对则退回显示帧投影），所以
  16 份真实 LR sidecar 的字节往返一字不动。副产品：`C-rotation-skeleton.md`
  记的「自己写的文档自己读不回旋转径向」往返洞**就此关闭**，有测试钉住。
  **⑧ 供给侧**：新 `decode::source_frame` 一次元数据读同时给出**未转尺寸**与
  文件自己的 EXIF 方向（`frame_size_turned` 改为它的薄封装，去重一处）；
  `pipeline::photo_frame_aspect` 从「显示尺寸」改为「源尺寸 + 合成方向」——
  竖幅照片此前 `s = W/H` 直接倒过来了。取画幅的闸门从「有旋转径向」放宽到
  「有画幅坐标或有拉直」，代价是每张照片每进程一次**记忆化**元数据读。
  **⑨ 解析器加固（P5 §8）**：JPEG 的 XMP 走 APP1，**用户图库里 13 份导出**
  （含 P3 七张之一的 `DSC09024_1.jpg`）超过 64 KB 段限、拆进 GUID 键控的
  ExtendedXMP 续块——本轮之前 `.jpg` 掉进 TIFF 臂、`GenericTiffReader` 在
  `FFD8` 上失败、答案**静默返回「没有」**（正是该函数契约明令禁止的沉默）。
  新 `decode::jpeg_xmp_packet` 按**偏移**重组、只认标准包点名的 GUID、链上有洞
  就报错而不返回短文档；`store::embedded_packet_for_restore` 的 `is_raw` 闸门
  不动，所以恢复行为零变化。P5 的另一半（`crs:RetouchAreas` 自带
  `Mask/Ellipse`，全包 `Mask/` 计数会与按 correction 的解析在 166 张里差 83 张）
  **本引擎一直是按 correction 作用域的**（`mask_summary` 走
  `crs:MaskGroupBasedCorrections`）——补一条回归测试钉住它别漂。
  **⑩ 发射规则对齐（P2 §4.3；纯字节层、行为中性）**：LR 的细节/降噪伴随键是
  **按幅值**成组写的（211 份导出里 `LuminanceSmoothing>0` 的 4 份全带 Detail、
  =0 的 207 份全不带；159 份带 `SharpenEdgeMasking="0"`）。本引擎按**每键自身
  非零**发射，于是 `DSC09533.JPG` 那种形状（Smoothing=50 / Detail=50 /
  Contrast=0）会漏掉 Contrast——LR 从不写这种文档。修法**只收 ACR 默认为 0 的
  两个伴随键**（`LuminanceNoiseReductionContrast`、`SharpenEdgeMasking`）；默认
  非 0 的四个（Radius 1.0 / Detail 25 / 两个 50）继续**以缺席表达「没学过」**
  ——写 0 会让 Lightroom 真的按半径 0 去锐化，那是渲染改动不是拼写改动。
  **门**：652 lib（+10）/ 9 CLI / 131 GUI / 2+2 合约，`cargo clippy --all-targets`
  与 `--features gui` 双 0，audit_i18n 全 9 门 0（**无新 UI 词条**——新披露走
  CLI/stderr，与既有 unparsable 提示同通道，字体 cmap 无需再生成），
  `AUTOSHOP_LR_PROBE_FIXTURES` **16/16 仍字节相等**（并新增裁剪六键与 tiff 三键
  两组断言），`AUTOSHOP_RAW_ZOO` 9/9 绿。**7 条变异**逐条验证红→回滚。
  **⚠行为变化需知悉**：①带拉直的 LR 导入方向**从此反过来**（此前是错的）；
  ②带拉直+裁剪的 LR 导入可能被裁到内接帧（已披露；实测最坏 0.55 %）；
  ③竖幅照片的 LR 蒙版/裁剪**第一次落在对的地方**；④我们自己写的 sidecar 从此
  带 `tiff:` 三键（仅限新写文档，以及「底稿完全没提 tiff」的合并）。
  **登记不修**：`quarter_turns` 本身进不了经典 ACR sidecar，所以对**用户手动
  转过**的照片，sidecar 声明的是合成方向、而再导入回来的 recipe 是
  `quarter_turns=0`——几何对 Lightroom 是对的（LR 不知道我们的转数），但**我们
  自己**再导入会与渲染时的显示帧差一个四分之一圈；权威恢复路径是 store 里的
  recipe.json，sidecar 是给 LR 的。GUI toast 通道仍未接新披露（沿用 Batch-2 的
  既定现状：新披露只走 stderr）。
  **本批不消费、但同轮产出的两条测量，记在这里免得丢**：
  **(a) P4「画笔 alpha 核」＝阴性判定（HIGH）**。用户图库的（配方,像素）对
  **不能**分辨核形状，也不能分辨叠加律，L-08 的画笔臂**仍钉在一次 LR 实验上**。
  三条路各自撞上可量化的障碍：A/B 参考渲染——5 个有多份导出的拍摄里 **0 个**
  切换过画笔 correction；孤立单点——86 条单点笔画（去重后 32）**0 条**孤立
  （足迹未被其他点覆盖的比例中位数 0.000、最好 0.118）；像素反推——最好逐像素
  S/N **1.24**、中位 **0.50**，且整体「检出」在位移模板对照下消失。根因是一个数：
  28 条带 Paint 的 correction 里，背景自身在笔画尺度上的 ln-luma 结构
  `σ_tex` 中位 **0.570**，而全库最强的画笔调整只有 `Δ = 1.317` ln-luma
  ——**没有一条同时满足「调整够强」与「背景够平」**，两者在真实修图里
  反相关。**用户已批准那次画笔核实验**（in flight，素材未到）。
  **(b) P2 的 L-06/L-05**：见上方 v0.32.0 条「待办」行的就地更新。

- **v0.32.0（2026-08-19，第二十六轮 = 径向蒙版几何根修）** ——
  素材＝**用户为此专门拍的 12 张受控 Lightroom 导出**（M-B ①结清）
  ＋对导出成片的像素级测量；证据档案
  `~/.claude/plans/r25-materials/lr-experiment/`（`BBOX-DECODE.md` 角点模型、
  `ANGLE-MODEL.md` 角度与枢轴、`probe2/PROBE2-VERDICT.md` 八帧表、
  `probe3/PROBE3-ADDENDUM.md` 落笔律、`probe4/PROBE4-FINAL.md` **定稿**）。
  **V2_PLAN §7 item 1 首次判 CLOSED**（剩余的 Roundness/Midpoint 拆为 item 11）。
  六项：
  ① **角点解码（导入）**。`crs:Top/Left/Bottom/Right` 不是外接框，是椭圆
  自己那个框在**像素帧**里的**旋转后两个对角**：`X=(R−L)/2·W`、
  `Y=(B−T)/2·H`（**带符号，绝不 abs**），`a=X·cosθ+Y·sinθ`、
  `b=−X·sinθ+Y·cosθ`；守卫 `a>0∧b>0` 违例具名拒收该 correction。零自由
  参数的符号律（`Left>Right ⇒ Angle>0`）图库 16/16 契合，p=2.5×10⁻⁵；旧的
  天真读法轴比中位差 1.84 倍（p90 4.86、max 40.7），195 个分量里 16 个根本
  读不出来。
  ② **`crs:Angle` 不再扣留（写回）**。符号/枢轴/**幅值**三项均已实测，
  因此 v0.31.x 「导出未旋转椭圆+损失注记点名角度」的立场退役；折算走
  ANGLE-MODEL §6.1 的 SVD（像素帧倾角 ↔ 引擎归一化帧旋转，两者在真实
  ±44° 域内差达 11.2° 屏幕倾角）。**16 份真实 LR sidecar 四角字节级往返**
  （含 LR 自己写的 `Left>Right` 排布），角度往返 <10⁻⁴ °（上界由 recipe 的
  f32 存储决定）。注记变窄为唯一剩余情形：**文档没声明画幅**。
  ③ **落笔两端点（渲染，全局统一）**。曝光梯子反推真实蒙版权重（五帧，
  长宽比 1.03…7.46，含一帧旋转、一帧角落放置）定下 `d_in=k(1−f)`、
  `d_out=k(1+f/2)` 三次平滑（拟合系数 0.99/0.45 对律的 1 与 ½，chi2/dof 1.52
  零自由参数 vs 1.37 两自由参数）。旧引擎外沿在 `d=1`，实测 f=0.5 时在 1.25
  —— **蒙版小了 29%**。`k` 属于 XMP 边界的坐标帧常数，已在几何里折完，所以
  引擎内不含 `k`，引擎原生蒙版与导入蒙版同一条落笔律（用户拍板）。
  ④ **DefaultCropOrigin 收编渲染窗口**。块配准实测每张 ARW 渲染偏 LR 帧
  `(+31±6, +20±1)` px 纯平移；ARW 的 `DefaultCropOrigin=(32,20)`。根因是
  rawler 0.7.2 两事相乘（ARW 的 `active_area` 从 `SonyRawImageSize` 建、原点被钉在
  (0,0)；`CropDefault` 只在 `crop.d != intermediate.dim()` 时才裁，而纯平移永远
  不满足）。修＝把去马赛克 ROI 移到声明的矩形（`decode::align_default_crop`），
  零额外拷贝。**⚠全局像素影响：每张 Sony ARW 渲染从此位移 (32,20)**。
  ⑤ **`crs:LocalHue` 尺度 ×180**（实测锚点：UI +50 ⇒ 文件 `0.277778`，
  ×180=50.00004）；读侧 ×180、写侧 ÷180、域门同步（否则会放行滑块端点以外
  的值）；recipe 域仍 ±100。V2_PLAN §7 item 10 的 LocalHue 半边关闭，
  LocalSharpness 仍开着。
  ⑥ **极性真值表四行钉死**（v0.31.2 语义已被像素证实，本轮只补合约测试）。
  **recipe schema 零新字段**（旧 exe 不会因蒙版硬拒新 recipe），但**三类行为变化
  需知悉**：①LR 径向首次形状/角度/位置正确导入；②引擎落笔外沿变宽，
  **旧配方的径向渲染会漂**（用户已接受；版本快照可回溯）；③渲染窗口对齐 LR。
  门：620 lib（+10）/8 CLI/129 GUI/2+2 合约，clippy 双配置 0，audit_i18n 全 0，
  字体 cmap 801/801。新测试均做变异验证（见各测试 doc 的 MUTATION 行），另有
  `AUTOSHOP_LR_PROBE_FIXTURES` 环境变量门的真实 sidecar 夹具测试（未设则静默跳过；
  实跑 16 份全通）。发版：feat b4a6c35，tag v0.32.0→b4a6c35，assets 回下载
  字节验证=本地逐位相等：cli
  46c474a6901d8280ac2cabf4508a340ba843213c184714d18f2a70d0a4660812
  =29,108,942 B / gui
  6f6b388664b46c6bb4cde4da2e207cf6ea90c7f7444dc2fb0027ed816b209a19
  =38,583,788 B。**待办**：~~90° 旋转骨架仍留 v0.33~~ **← R27 Batch-2 已落地
  （引擎侧 A1-A6+A9+A10；XMP 侧 A7/A8 仍等竖幅实测）**~~；
  竖幅/裁剪帧~~ **← R27 Batch-3 全部关闭**（A7/A8 落地；竖幅帧由 P1 判死、
  裁剪帧由 P5 判死，两者都不需要新导出——用户图库里已有的成片就够，见本文件
  Batch-3 段⑤⑥）；`|Angle|>45°`
  未验证（已在码内登记触发条件）；半轴尺度分歧（1.0325 vs 1.0065）需两张
  `Feather=0` 导出才能定（**R27 P2 独立复现了这条分歧本身**：1.00658±0.00002
  vs 1.03235±0.00003，并排除了「长宽比是驱动因素」，L-05 因此**留在原登记**，
  只是实验规格更锐利了——两张居中、24 mm 与 105 mm、半轴约帧宽 15 %）。

- **v0.31.2（2026-08-18，证据修复批 —— 用户自有图库只读取证再证伪两条既定
  读法）** — 素材来源＝对用户图库的只读普查（201 径向全库 + 7 份 M-B sidecar
  独立复算）+ E1-verdict 像素级测量（证据档案
  `~/.claude/plans/r25-materials/web-evidence/`，E1-verdict + multimask-forensics）。
  三条：
  ① **`crs:Flipped` 与反相解绑（主项）**。R24 曾据单例判两字段「独立并存」——
  R25 全库普查证伪解读：201/201 径向 `Flipped` 与 `MaskInverted` **完美反相关**
  （155 `(true,false)` + 46 `(false,true)`，相同值组合 0 例；M-B 复算 23/23，
  线性渐变 27/27 根本不带 `Flipped`）。两批证据字节层不冲突（「不是镜像副本」
  与「值恒相反」同真），死的只是「所以含义不同」的解读：LR 把**一个**反相位
  写了两遍。本引擎 `Radial::flipped`（render.rs 分量权重取补）与
  `LocalAdjustment::inverted`（合成权重取补）按 XOR 合成，两半都读＝值与其补
  XOR＝**每个导入 LR 径向被无条件反相**（155 个错、46 个碰巧对）。修＝导入只认
  `MaskInverted`（`flipped` 恒 false），写侧两属性都从净反相
  `lr_net_inverted = flipped ^ inverted` 派生——只写 LR 自己会写的组合，故在
  「LR 渲染器读哪个属性」两种读法下都成立。实测 `DSC09568` 对真 LR 导出
  tone-matched RMS **0.1099→0.0751**（蓝 0.1901→0.0869，引自 E1-verdict §6，
  本批未重跑像素）。**recipe schema 零改动**（无新字段＝不触发 v0.31.0/1 exe
  蒙版硬拒）；我方 `flipped` 仍渲染、仍是 GUI 翻转勾选，既有 recipe 渲染
  一像素不变。**接受成本（复审裁决在案；用户 2026-08-19 复核＝定案，L-19）**：
  往返后净反相精确保留，但承载位
  从「翻转」并入「反相」勾选（XMP 只有一位，投影必然塌陷；替代方案加
  carried-only 字段会改 schema 触发前向硬拒，判不值）。≤v0.31.1 我方写的
  带翻转 sidecar 现读回不反相＝与 LR 读法一致化，非新损失。
  ② **两处注释旋向改正**（render.rs:1930 段 / recipe.rs:940 段，只改注释码
  不动）：旧注释称引擎径向角「逆时针，y-down 屏幕意义」，实测**顺时针**
  （+30 渲出右端朝下；用户 5 照符号检验 5/5，p=0.031，**supported 级未到
  proven**）。`R(+θ)` 在 y-up 数学系确为逆时针——旧注释带 y-up 读数却声称
  y-down。
  ③ **多蒙版 Correction 取错底形**。旧选择器按**种类**取几何（`Mask/Gradient`
  恒优先于 `Mask/CircularGradient`）且无视 `crs:MaskBlendMode`：字节级证实
  两例意图反转——`DSC08960` 蒙版 3 与 `_DSC9583` Mask 9 均为「径向底形+线性
  减法」，旧码留下 LR 用来**抠掉**的形状当整个蒙版。修＝新 `base_geometry_at`
  优先首个默认 BlendMode 组件（全减法才落回首个）；`Rotation` 损失注记只对
  **实际导入**的几何发（旧码 `DSC08960` 四条注记三条描述从未入 recipe 的
  径向）；`BlendMode` 注记保持组件级（对被弃减法组件是真话＝v0.31.1 的披露）。
  实测恢复 3 蒙版（`DSC08960` 径向 {3}→{1,3,4}、`_DSC9583` 8→11 全入）。
  **登记不修**（注释带触发条件+去向钉在代码）：~~CLI 零蒙版披露（`main.rs` 不调
  `describe_import_losses`，通道设计缺口）~~ **← R27 Batch-1a 已修（L-07）：
  `main::lightroom_import_note` 在 `analyze`/`auto`/`match` 三处随 `xmp ->` 行
  发 stderr，与导出侧损失行同一通道；`eval.rs` 仍只数 drop，这是标尺定义不是通道
  缺口，已成文**；`unknown_component` 预占形状账
  （混合 AI/画笔+参数化几何的 Correction 整条拒收正确但 6 个可表示形状
  无披露，披露粒度设计题）**← 用户 2026-08-19 裁决＝根修实现，先取证三类组件
  字节层，属 R27 后续批**。
  门：clippy 0 双配置；**610**(+4：`lightroom_spells_one_inversion_bit_twice`
  改判+`our_own_flip_leaves_as_lightrooms_own_inversion`+
  `a_multi_component_correction_imports_its_base_geometry`+
  `only_the_imported_geometrys_rotation_is_disclosed`+env 门
  `real_lightroom_radials_carry_one_inversion_bit_spelled_twice`）lib / 8 CLI /
  129 GUI / 2+2 合约双配置；audit_i18n 全 9 门 0；无新 zh 词条；6 项变异检查
  修前必红（主审第一方抽查复现）。发版：fix 2a382dd，tag v0.31.2→2a382dd，
  assets 回下载字节验证=本地逐位相等：cli
  0e2e129133c66ff171f18ae53a673cf6c22817dbc407668d850618c9b663350a
  =29,144,638 B / gui
  e542146d59bdeed38e5a7ca071e79e15935bdbf14949ea5fc2f33e01e99753e2
  =38,616,119 B。

- **v0.31.1（2026-08-18，证据修正补丁 —— 三条「锁定假设」被网络取证证伪后的
  根修）** — 素材来源＝对 GitHub 公开 `.xmp` 的只读普查（证据档案
  `~/.claude/plans/r25-materials/web-evidence/`，E1-E5+MANIFEST）。**取证文件
  本身不入库**，夹具一律合成、出处写进注释。三条：
  ① **`crs:Sharpness` 是 1:1，不是 0..100**（主项）。LR「细节 > 锐化 数量」
  滑杆 UI 上限就是 **150**，sidecar 原值存储：15 份真实 sidecar 带
  `crs:Sharpness="150"`（`maxbordogna/finger_counting` 14 份 NIKON Z 6
  `crs:Version="15.3"`＋`ninjahisser/Fotos` 1 份 NIKON Z 30 `"17.2"`，两仓两机
  两代），566 次观测的直方图最大值＝150（E2）。旧的 0..100 出自第三方 Lua
  滑杆表（`RobColeLr/…/DevelopSettings.lua` L165），非 Adobe 出处，被 sidecar
  直接推翻。旧代码四处同源：读侧 `×1.5`、写侧 `×⅔`、`crs_number_is_in_recipe_range`
  的 `Sharpness` 特判 `(0,100)`、以及把 `100×1.5==hi` **钉成锁定断言**的测试。
  修＝读写全 1:1，**特判与锁定断言整体删除**（注册表行 `range=(0,150)` 本就
  正确，P0 的派生带宽随即自然成立——特判死于证据，是这个修的形状）。实测前后：
  `Sharpness="40"` 导入 60→**40**、`"35"`→52.5→**35**、`"150"` 曾被**误报为
  不可解析数字**（一份普通 LR 文件被告知损坏）→现在无告警；写侧渲染 60 曾写成
  `"40"`、150 曾写成 `"100"`（旧写法根本**说不出** 150）→现在所见即所写。
  **旧配方不迁移**：recipe 的 `sharpening` 驱动的是我方 σ 引擎（语义本就 ≠ LR），
  引擎渲染一像素不变；旧 ×⅔ 的真问题是「渲的 60 写成 40」的 WYSIWYG 错位。
  ⚠ **eval 口径变更**：ruler 的 `Row.scale` 字段（只为这一条而存在）删除，用户
  侧 `sharpening` 从此按原值读——**R25 的 M-C 147 张基线里 `sharpening` 一行
  与新报告不可比**（旧行把用户值读高 1.5×），其余行不受影响。
  **← L-31 已结清（2026-08-19 第三次尝试）**：这条不可比性连同另外两条（R23
  提示词全档重写、v0.32.0 的 (32,20) 像素位移）已由 HEAD `f43dd85` 的 147 张
  全量重跑一次性作废——中转端点 + `--jobs 3`、147/147 双流零回退、EXIT=0；
  此后唯一可引参照＝`~/.claude/plans/r27-materials/mc-eval-147-r27.txt`，
  R25 那份降为历史（数字对照见 M-C 行）。
  ② **Subtract 组件不再整条拒收**。`crs:MaskBlendMode="1"` **恒伴**
  `crs:MaskValue="0"`（E4 全量普查：157 文件 479 个实例，正则与
  `xml.etree.ElementTree` 双路复算、0 解析失败；26/26 无反例，反向 0/479），
  即这是 LR 的**减法编码**而非「哑音蒙版」。旧读法先判 `MaskValue≠1` 就 `Err`，
  几何臂再把 `Err` 变成 `OutOfModel`＝整条 Correction 丢掉。修＝
  `BlendMode` 非默认**且** `MaskValue==0` 时取基础几何 + 记一条 `BlendMode`
  披露；**0 绝不当强度预乘**（强度只来自 `crs:CorrectionAmount`）。哑音
  （`MaskValue=0` 配默认 mode）与真部分值（普查中唯一的 `0.662178`）仍拒收。
  *范围登记*：普查里 26 个非零 mode 全落在 `Mask/Image`/`Mask/Aggregate`/
  `Mask/RangeMask` 上，**parametric 几何上零实例**——前两者仍走
  `unknown_component`→`Unrepresentable`（未变）。故此修补的是代码洞（用户把
  线性渐变复制成 Subtract 就会撞上），不是普查里已观测到的丢失。
  ③ **`crs:Roundness` 是 ±100 整数滑杆**，不是 0..1 的 bbox 长宽比。24/24 真实
  径向写裸整数、全为默认 `"0"`，与 `Feather="+100"`/`Midpoint="+50"` 同为整数
  族（E1）。旧读侧域外即整条拒收（几何臂）＝潜在拒收洞；修＝读侧门与
  `Recipe::clamp` 同步放宽到 ±100。**不做换算**：roundness 是 carried-only、
  `mask_weight` 从不读它，所以歧义值 `1` 无需裁决——按原值携带、按原值写回，
  两种读法都拿回自己的数字（与必须猜的 `feather` 的分野成文在案）。像素含义
  仍未测（V2_PLAN §7-1 保持 OPEN，同一次 LR 实验可闭）。
  门：clippy 0；**606**(+3) lib / 8 CLI / **129** GUI / 2+2 合约双配置；audit 0；
  三条新测试在 `3bd0a8c`（修前）上逐一必红（临时 worktree 实证）。发版：fix
  a1edb5c，tag v0.31.1→a1edb5c，assets 回下载字节验证=本地逐位相等：cli
  a40facb593e2d6d877f038904c5bf921f48d4e494714e738f61a88e861f2f047
  =29,145,300 B / gui
  dd237608913a46dc717c7fdb61d85cfacfc8911cc2f21b0da6934fc7b41774e6
  =38,615,557 B；`--version` 自证 0.31.1。

- **v0.31.0 RELEASED（2026-08-18，第二十五轮=收尾闭合轮：Lightroom 差距全量
  清账）** — 用户令「剩余的所有缺口、计划中的待办项，全部设计好并打磨+推进，
  彻底完成收尾，闭合项目」。八包收官（P0-P6 七个实现提交 + 轮末复审修复批）：
  `a398ea4` 基座三修 / `4eb54aa` 导入解锁 / `1ddf53e` B2 Texture / `98b4c65`
  B3 Detail·Defringe·CA / `3ae7df7` B4 PassThrough / `a98a82f` B5-B1 几何写回
  / `a1bd81b` P6 局部点曲线 / `464008a` 复审修复批。测试 543→**603 lib** /
  8 CLI / 114→**129 GUI** / 2+2 合约双配置，clippy 0，audit_i18n 0，字体门
  801/801。**`EditRecipe` +29 顶层字段**（B2 十 / B3 十七 / B4 `passthrough`
  一 / 修复批 `schema_era` 一），`LocalAdjustment` +4（四条点曲线），
  `MaskGeometry::Radial` +2；控件注册表 34→**63 全局** / 24→**28 局部**。
  **头号发现＝缺陷不是差距（P1，`4eb54aa`）**：用户 Lightroom 库里的**每一个**
  蒙版此前在导入时被整条丢弃——`xmp.rs` 两道门任一命中即丢：`crs:Angle` 存在
  （LR 每个径向都写，未旋转时写 `"0"`）、`crs:MaskBlendMode` 非我方文件即拒
  （160 份取证里 620 顶层 + 428 组内 = **1048/1048** 个蒙版组件全带）。与
  `INERT_LOCAL` 非零硬拒、未知 `crs:Local*` 硬拒等**共六道同源门**是一个根因：
  导入侧「不认识就整条丢、只给一个整数」，而导出侧自 R22 M6a 起早已是具名
  `MaskLoss{name,reason}`——**这个不对称就是根因**。修＝`MaskImportReason` 十变体
  与导出侧同文件同习语（八条**有损但导入**：旋转角度/混合模式/未建模滑杆/未知
  `Local*` 键/附加形状/外部值域蒙版/读不出的局部曲线/曲线精修饱和；两条 **drop**：
  `Unrepresentable`＝没有参数几何可站（画笔·AI·深度）、`OutOfModel`＝读得懂但超
  出模型——**分开才不让句子说谎**），几何照常导入、未建模项具名披露。真实回归（7 份
  42 条 Correction）：**修前导入 0 → 修后 31 导入 / 11 拒收**，
  `imported+refused==corrections` 每份成立；仍拒的是 `Mask/Paint`·`Image`·
  `Aggregate`·`Ellipse`（LR 靠 digest 重算，第三方取不到像素，披露原话点名）。
  **P0 基座三修（`a398ea4`）**：把 B2-B5 每批都要重改的三张手写表派生化——导入
  带宽表从注册表派生、● 指示器由 `family_is_active` 派生（`DOT_EXEMPT` 唯一豁免
  带理由）、`MaskLossReason` 两处硬编码数组由 `ALL` 派生；**拒改
  `PIPELINE_BYTES_PER_PIXEL`**：R22 复审登记的「既往偏低 ~17%」经第一手三时刻
  算账＝**恰 24 B/px**，非实测而是当年的主张，算账入测试
  `develop_peak_accounts_for_every_pass`，**该登记就此关闭**，P0 零行为变更。
  **B2（`1ddf53e`）**：全局 `texture` 与蒙版路径**同一算子同一半径式**（位级孪生
  钉：全覆盖蒙版 texture ≡ 全局 texture），插在 clarity 与 saturation 之间照 ACR
  Basic 面板序；裁剪后暗角六 + Grain 三走 CarriedOnly；「暗角」同名冲突改
  「镜头暗角」；工单外自堵一处真缺陷——merge 现在剥我方键，不 carry 就会删掉用户
  sidecar 里的 grain/暗角。**B3（`98b4c65`）**：Detail 八轴 + 手动 CA 折进已有的
  每通道径向因子（`MANUAL_CA_PER_UNIT=2e-5`，带完整推导；`geometry_profile` 单
  漏斗铺满全部几何消费点）+ `AutoLateralCA` + Defringe 六键；Defringe 默认
  `0/30/70/0/40/60` ＝**本仓首批非零默认**（读侧缺键回落字段默认非 0，否则旧文件
  未动就变成非 noop）；`i18n` 里在案的「De-fringe in a later batch」承诺原地删除。
  **B4（`3ae7df7`）**：`Tier::PassThrough` 的**首个真载荷**——`PASSTHROUGH_CRS`
  16 键（Perspective/Upright 八 + `CameraProfile` + Calibration 七）`String→String`
  原样进出（三种中性拼写 `"0"`/`"0.0"`/`"0.00"` 共存＝String map 的实证理由）；
  新增对称披露 `global_render_gaps`（LR 渲染而我方不渲染的那半，是 SF4-C 默认档
  的必要配套）。**B5-B1（`a98a82f`）**：`crs:Midpoint`/`crs:Version` 读写往返
  （字段级 serde default，照 `coord_era` 先例）；旋转损失**载荷化**——报「旋转
  37° 未写入 XMP」而非「有旋转」（实测库内 37/−22/2/−44/−24/−2）；`crs:Angle`
  仍不发射＝在读不懂的刻度上断言方向不如让 LR 用自家默认；拆掉一颗真雷：
  `range_mask_xml` 在同一 Correction 内几何组件之后写 `crs:Version="3"`，无界扫描
  会把它当椭圆版本戳读进每个带值域的径向。**P6（`a1bd81b`）**：局部点曲线四条
  （LR 裸名 `MainCurve`/`RedCurve`/…，**逗号后无空格**，与全局曲线 `"67, 61"` 的
  格式化函数刻意不共用，反合并变异双向钉）；走已有融合通道零新 pass。
  **轮末 Opus 5 对抗复审判定 BLOCK → 修复批 `464008a`（一根因两 HIGH，均对
  v0.30 前后在真实 sidecar 上实证）**：根因＝**「配方没见过的键被读成用户清空」**。
  ①`preserve_masks` 的守门旗只在**有损**导入置位 → 无损导入 + v0.30 形状的无蒙版
  recipe 存盘**静默删光** LR 整个蒙版块（2/7 参考 sidecar 的 12/42 条被毁、注记为
  空；「导入修得越好、文件越没保护」）；修＝判据换成「**底片本来就有蒙版块**」，
  并删掉那个只写不读的破代理字段。②merge 的无知律只套了 B4 的 16 键，B2/B3 的 27
  个标量键仍走平坦律 → v0.30 recipe 一存盘就剥掉 Grain/PostCrop/SharpenRadius、
  把 `crs:Texture="-20"` 归零（3/7 真实照片中招，零披露）；修＝新字段
  `schema_era`（`coord_era` 先例逐条对照；0＝v0.31.0 之前）+ 派生集合
  `era_suppressed_attr_keys`（24 个 CarriedOnly 行 + `texture`/`ca_r`/`ca_b`，
  恰 27 键，PassThrough 因其 map 存在律更强而排除），**按键而非按配方**：旧照片
  上真拖过的值仍会写出，因为那是声明。其余：Look 交叉名作用域读法只会少读不会
  升级（原本会把 Look 里烘焙的 `Clarity2012` 提成用户滑杆）、paste 不再跨照片带
  `passthrough`、zh 误译修（`Camera profile`＝相机配置文件，镜头配置文件留给真正
  的镜头档）、P0 oracle 两条死析取补活、`tone_active`/`crop_active` 派生化收官
  （最后两组手写 ● 元组消失）、P6 曲线注释按实测容差软化（最差 1.5e-5 个 8-bit
  码）、env-gated 探针读不动夹具改为点名 panic 而非静默不计。**接受成本（已裁决
  并登记；用户 2026-08-19 复核＝定案，L-19）**：在 Autoshop 里删光蒙版**不再**把删除传播到 sidecar（要删请在
  Lightroom 侧删）——本轮主题就是数据保全高于删除手势；已编辑过的蒙版块 republish
  会按我方 writer 确定性重铸 `MaskName`/`MaskSyncID`。**⚠ 前向三类语义（互不相同，
  发版说明分列）**：①`EditRecipe` 顶层 +29 字段 + `deny_unknown_fields` →
  **v0.30 exe 硬拒**任何 v0.31 写的 `recipe.json`（同 v0.29→v0.30 `coord_era`）；
  ②`LocalAdjustment` +4 曲线且同样 `deny_unknown_fields` → **带蒙版**的 recipe 被
  旧 exe 硬拒，无蒙版字节不变（同 R23 LocalHue/Sharpness）；③`MaskGeometry::Radial`
  +2 是**内部标签枚举、无** `deny_unknown_fields` → 旧 exe **静默丢弃再写回**＝悄悄
  改文件，**本轮首次出现的第三类**。**← R27 Batch-1a 关闭（L-17）**：「serde 不
  支持在内部标签枚举上 `deny_unknown_fields`」这句是**错的**，实测（serde 1.0.228
  + serde_json 1.0.150）加在**容器**上即生效且自动豁免标签字段。`MaskGeometry`
  与 `RangeMask` 两个内部标签枚举现已都加上，**第三类从此与①②同为响亮拒收**；
  代价是 v0.33 起旧 exe 遇到新版蒙版几何会按字段名拒收整个 recipe，这正是
  「不支持降级」的成文姿态。指纹变 → R21 已删版本登记簿的结构臂一次性
  以**新号**重新保档（hwm 臂不动＝同号永不复活，合约 2/2 绿）。**M-C 全量 147
  重基线（`~/.claude/plans/mc-eval-147-full.txt`）**：147/147 零失败，**总 gap
  14.2%**；最大系统性分歧 whites **+22.55** / blacks **−14.18** / highlights
  +11.69 / noise_reduction +11.71（AI 恒加降噪，用户不加）/ vibrance +10.80 /
  clarity +8.12 / contrast **−7.61**；主曲线 RMSE **6.1**/255、RGB 三通道
  0.6-0.7；**补偿模式**＝AI 在曲线里打对比（S 强度 13.7 vs 用户 3.7）同时在
  contrast 滑杆上收着打；temperature_k mean|Δ| 401K、bias **−163K**（AI 偏冷）；
  saturation 被 AI 略过 52/147。**蒙版行是 §0 缺陷的独立实证**——eval 自己数出
  用户侧「0.00（+2.35 我方导入器拒收）」，即平均每张 2.35 个用户蒙版被拒，正是
  P1 修掉的那个数字。判读与「据此调提示词」归用户，本轮如实呈报不预改。
  **两条台账口径更正**：①「803 字形子集」是历史标签——实测
  `NotoSansSC-autoshop.ttf` ＝ **751 字形 / 739 码点**，五字体 cmap 并集 **2396
  码点**（`ROADMAP` 旧句已改指「成品字体子集」）；②R22 登记的
  「`BYTES_PER_PIXEL=24` 既往偏低 ~17%」以第一手算账**关闭**（见 P0）。
  **登记不修**：~~AI 精修会把 `midpoint`/`version` 重置回 50/2（库内全部径向本就
  50/2＝**无观测实例**；修法要动有 bug 前科的整体回退启发式，不值）~~ **← R27
  Batch-1a 已修（L-09，用户 2026-08-19 裁决＝修）**：`pipeline::
  carry_radial_carried_attributes` 独立一遍从精修底片把两个携带字段按名字回贴，
  **不碰** `schema_loses`（那才是决定整体回退的谓词，也正是当年判「不值」的理由），
  合成夹具用非默认 `Midpoint=30`/`Version=3`；~~`Sharpness`
  0..100 假设被当作锁定不变量（需用户一份 >100 实测样本才能闭）~~ **← v0.31.1
  已证伪并根修，见本文件 v0.31.1 段**；~~`nr_detail`/
  `nr_contrast` 零观测形态（全库 `LuminanceSmoothing=0`，中性有推理无实测）~~ **← R27 P2
  已实测结清（L-10）**：211 份 Adobe 导出里 `LuminanceSmoothing>0` 的 4 份**全部**带
  Detail、=0 的 207 份**全部**不带；`ColorNoiseReduction>0` 的 133 份全带
  Detail+Smoothness、=0 的 64 份全不带——R25 登记表声称的每一个默认值都被图库佐证、
  没有一个被推翻（`nr_detail`=50 四中四、`color_nr_detail`/`smooth`=50 133/133、
  `sharpen_radius` `+1.0` 160/160、`sharpen_detail`=25 159/160）。**同一份取证暴露了
  一处纯字节层分歧**：共写规则是**按幅值**而非按每键自身非零，已在 Batch-3 ⑩ 修；
  Effects 段目前落在「Detail & Lens」弱组标题下、标题未扩；store 指纹 fail-open
  已披露。**后续登记**：~~**90° 手动旋转＝v0.32 首项**（骨架见下）~~ **← 归属更正
  2026-08-19：v0.32.0 被径向几何根修占满，90° 顺延到 v0.33（与 v0.32.0 条的
  「仍留 v0.33」一致，本行是滞后行）**；~~B5-B2 角度写回、
  LocalHue/LocalSharpness 的 LR 数值尺度、Defringe 乙案（渲染近似）、
  `AutoLateralCA` 验收——四项都等**同一次** LR 实验~~ **← 该实验 2026-08-18 已做，
  四项分别落定：B5-B2 角度写回 ✅ 合于 v0.32.0；LocalHue ✅ ×180；LocalSharpness
  ✅ **÷100（当日先误判证伪、后经 Adobe 双锚定案，弧线见勘误条⑧与 V2_PLAN
  §7-10）**；Defringe
  乙案 = **不做**（甲案 CarriedOnly 终态，B3 行 + SF4-C 早已裁过）；`AutoLateralCA`
  ✅ 素材 `_DSC9596.xmp: AutoLateralCA="1"` 已在手**；底部变体条物理合并搁置；
  SF9-B 版本捕获像素不做；保存侧蒙版丢失红 toast 保持不降级；蒙版 Temp/Tint 照 LR
  留 Color 组。~~**素材欠（用户侧一次 6 合 1 LR 实验即可全取）**：Defringe 紫 +3 /
  绿 +2 → 加一个径向旋 +30° → 该蒙版 Hue +50 / Sharpness −40 → 复制该径向做
  Subtract → 打开 Auto CA → Ctrl+S 存 sidecar。~~ **← 六项 2026-08-18 全部交齐，
  逐项 file:value 见本文件「台账勘误（2026-08-19，R27 Batch-1a）」条①。**
  发版：feat 012d420，tag
  v0.31.0→012d420，assets 回下载字节验证=本地逐位相等：cli
  e4de58dc2b92a1a625595d5f6d8a06a0d284ca1655c01992f5df2a2c45c599ef
  =29,145,146 B / gui
  05d04b70eed24cd0f1e7b8328f055142b4a0aa10e5039aadbea5517beffc9c71
  =38,613,218 B；`--version` 自证 0.31.0。

  **90° 手动旋转设计骨架（~~v0.32 首项~~ **← 归属更正 2026-08-19：v0.33**，
  本轮拍板推迟，骨架入库以免再丢）**

  | # | 位置 | 设计 | 落地状态（R27 Batch-2，2026-08-19） |
  |---|---|---|---|
  | 7.1 | `src/recipe.rs` | 新 `quarter_turns: u8`（0..3）。**不复用** `straighten_deg`（它是 ±45 且直接映射 `crs:CropAngle`） | ✅ 已做。`clamp` 用 `%= 4`（**同余类不是区间**：4 意为「不转」而非「最大合法转角」）；序列化用 `skip_serializing_if`（全库唯一一处——纪元戳「缺席」与「值为 0」含义不同故必须常写，而缺席的转数就是 0，跳过无损。收益：未旋转的 recipe 与 v0.32 写出的**逐字节相同** ⇒ R21 结构指纹**不需要**像 v0.31.0 那样做重归档）。`catalogue.rs` 注册表新增 `engine_only` 行（第六类理由：模型看到的**已经是**显示帧，问它「该转多少」只能得到猜测；required 字段还会在每次 Refine 返回 0，把照片转回去、却把裁剪与蒙版留在转后的位置） |
  | 7.2 | `src/render.rs` / `src/decode.rs` | 根因洞见：**EXIF 方向与用户四分之一圈可以合成同一个 `image::metadata::Orientation`**。改动＝一个 `compose(exif, quarter_turns)` + `raw_orientation_of` ~~三个~~消费点读合成值；`orient_f32`/`oriented`/`orient_point` 原样复用 | ✅ 已做，**但是五个消费点不是三个** ← 勘误：骨架写于 v0.32.0 之前，`frame_size` 是那一版新增的第四个，另有 `pipeline` 里基于路径的迁移点。`render::compose_orientation` 走 `to_flips`/`from_flips` 位代数（第一个映射发生转置时，第二个映射的两个翻转**交叉**再异或），穷尽测试对 9×4 全部 (态, 转数) × 四个探点核对它等于两次 `orient_point` 复合。`decode` 侧新增 `*_turned` 门（`decode_raw_turned` / `decode_any_turned` / `preview_only_turned` / `embedded_preview_turned` / `frame_size_turned`），旧签名**保留为「转数 0」那扇门**——多数调用者确实没有 recipe 可读，让它们各自默认 0 正是 v0.30 之前方向漂移的成因 |
  | 7.3 | 坐标 | 显示帧改变 → 存量 crop / 蒙版几何随转，**复用 `orient_recipe_coords`**（已 8 态全覆盖） | ✅ 已做，`pipeline::rotate_recipe` 一处统管。**传的是 delta 不是累计值**（骨架点名的双转危险）。四次 1/4 圈回到原点：栅格层面无损，坐标层面精确到 f32 舍入（~1e-7，非累积——`1.0 - v` 是一次 f32 减法，多转几圈也不增长） |
  | 7.4 | 栅格蒙版 | v0.30 的迁移刻意不转位图；**这次不同**——栅格是我们自己写的 PNG，`image::rotate90` 无损，应真的转（新号落盘，旧的按 R21 纪律） | ✅ 已做，且是**全有或全无**：先把所有栅格转进新号文件，一张读不出就整个操作报错、recipe 一个字段都不动（半转的显影——参数蒙版转了、手绘蒙版没转——下游无从分辨，正是 `backup_saved_develop` 拒绝的那类静默损坏）。旧文件留在原地（版本快照与别的 recipe 可能仍指向它） |
  | 7.5 | XMP | LR 的 90° 不是 `crs:CropAngle`（±45）而是 `tiff:Orientation` + 裁剪矩形落在旋转后的帧里；我们今天完全不写 `tiff:Orientation` | ⛔ **本批不做（A7/A8）**，卡在同一份竖幅实测上：写侧 `s = W/H` 与 sidecar 自报的 `tiff:ImageWidth/ImageLength` 必须一致，而 LR 对旋转帧怎么写那两个数正是 `xmp::FrameAspect` 已登记的未测项。`frame_size_turned` 已备好、`photo_frame_aspect` **仍走未转的那扇门**。后果登记：本 build 写出的 sidecar 描述的是**未转**的画幅 |
  | 7.6 | 缓存 | decode 缓存盐 3 → 4 | ✅ 已做，**但是两个盐不是一个** ← 勘误：骨架漏了 `style::CURRENT_INDEX_VERSION`，它的 v3 语义字面就是竖幅特征。缩略图盐 3→4 **并把转数本身加进 key**（旋转不改源文件，`(path, mtime, size)` 会永远命中横躺的旧条目）；风格索引 3→4 且建索引时按转数解码。两处共用新增的 `store::saved_quarter_turns`（廉价、无副作用、按 `Value` 解析以免新版 recipe 的未知字段把缓存 key 打成硬失败、任何失败回落 0） |
  | 7.7 | GUI | 工具栏 ~~`↶ / ↷`~~ 两个按钮，各一步撤销 | ✅ 已做，**但换了字形** ← 勘误：`↶`/`↷` 一格之隔就是撤销/重做，`↺`/`↻` 也早被「↺ 全部重置」「↺ 清除」「↻ 重画」占了 ⇒ 改用 `⭯`/`⭮`（U+2B6F/U+2B6E，全树未用，且已在 `NotoSansSymbols2-autoshop.ttf` 子集内，免再生成）。中文词条为过字体子集改写过：`逆 圈 命 落` 四字不在成品 cmap 内，故写作「向左/右转 90 度」「待用」「写入磁盘」 |

  **风险**：坐标帧是 v0.30 的根修主场，任何回归都会以「竖图横躺」复发；且
  `quarter_turns`（用户**意图**）与 `coord_era`（存储**纪元**）正交，不可合并成
  一个整数——`recipe.rs` 已为 `version` 写过完全同构的论证。**工作量**≈2 包。
  **← 落地补记（Batch-2）**：该风险由
  `a_quarter_turn_on_any_exif_state_transposes_the_dims_iff_it_transposes_the_pixels`
  兜住——8 态 × 4 转数断言「合成方向转置 ⟺ 渲染帧转置」，并顺带核对
  `decode::orientation_transposes` 与 `render::oriented` 的真实输出一致；RAW zoo
  探针（9 厂牌）另加两臂（四个转数的 `frame_size_turned` + 一次真转渲染），
  `AUTOSHOP_ORIENT_PROBE_RAW` 竖幅探针加了「+1 转 ⇒ 横幅、+2 ⇒ 仍竖幅」四行。
  实际工作量＝**1 包**（A1-A6+A9+A10），A7/A8 单列。

  **本批已登记不做的三条**（各有成文理由，不是遗漏）：
  ① **XMP 侧 90°**（见 7.5，等一份竖幅 LR 导出）；
  ② **带烘焙像素的照片不可旋转**——修复/AI 主图是一张按烘焙时画幅落盘的文件，
     本 build 没有「该文件早于某次旋转」的记法，转画布等于让主图在下次打开时
     重新横躺。GUI 两个按钮在整条变体带**任一**卡片带像素时禁用，并在悬停里
     说明理由（R24 #7 立的「像素态禁用带理由」规矩）；正确用法是**先转、再修**；
  ③ **web UI 的缩略图不跟随转数**——`serve.rs` 那条路不读显影库；桌面画廊已跟随；
  ④ **旋转后不能做像素修复**（修复 / 生成填充）——两个 worker 拿的是**照片路径**，
     经 `render::source_pixels` 重新显影（只应用 EXIF 方向），而画布导出的蒙版画在
     转后的画幅里；两个帧不同就会修错像素，且**烘焙进主图、任何滑杆都撤不回**。
     故**带理由拒绝**而非近似。把转数穿进 `source_pixels` 是独立一改：那是
     raw-vs-baked 的唯一分派点，降噪 / match / 反推都共用它。

- **v0.30.0 RELEASED（2026-08-17，第二十四轮=17 条反馈三轮清账收官之 R24：数
  据模型+方向根修+梯子定稿）** — 锁定计划之 R24 全量落地+素材自采反哺的两个
  计划外根修，六个实现提交（2e6a841 谓词+版本命名出处 / 7804e2f 应用到原图
  +ActiveWrite 写入面收敛 / 3449305 五档注册表+投递根 / 334f90a 轮末复审修复
  批 / 45c16dc 方向根修+坐标迁移 / e300a3a 联合梯子真实对定稿+M-D），测试
  518→543 lib/8 CLI/99→114 GUI/2+2 合约双配置，clippy 0，audit_i18n 0。
  **17 条反馈全部条目至此代码面收官**。本轮闭环：**#8**（版本命名+出处：
  variants.json v=1 additive 加 id/name+flatten 双向捕获；`.version-meta.json`
  咨询边车照 R21 非世代纪律，名字随号死烧号永不继承=合约二进制
  repro_version_names_follow_their_number；「＋存为版本」上卡=拍板②替代品；
  save_version Generated 守卫）；**#7**（应用到原图：参数覆盖 Original 卡、
  像素与源卡都留、一步 Ctrl+Z、像素态禁用带理由、Generated 快照载回底片的无
  标定真缺陷同修=reconcile_snapshot_calibration 双向单属主）；**#1**（词汇谓
  词 is_parametric 穷举收编 6 处+写入面 4+1 收归 store::variants_member
  ActiveWrite 四臂单属主（CLI 反推重开「▣原片」失真类从此有编译面出口；
  Analyze 写 Strip 顺手关闭在册跨面残留）+list_edits 合成视图+编辑态分组列
  表；变体卡改名 (photo,card-id) 键控+delete_variant 布防确认）；**#15b 基座
  +M8**（五档 tier 注册表 57 行全分类+两条包含断言从注册表/GUI 源码派生+披
  露三方向全派生——导出蒙版具名/导出全局=相机基调与镜头档首次有声/导入全局
  =补集宇宙自清空且**属性形+元素形双拼**；投递根 config::delivery_root 一等
  设置 Destination 信任+funnel 钉线；B2-B5+INERT_LOCAL 放宽待 M-B Defringe
  素材=用户 LR 补做）。**计划外根修①方向（#17 真闭环）**：rawler 0.7.2 对除
  DNG/QTK 外全部解码器把 RawImage.orientation 写死 Normal（上游源码
  `//cam.orientation, // TODO fixme`）→ 全库竖构图 RAW 缩略图/画布/导出三面
  横躺自 v0.11 起、且反推同帧检查误报「裁剪/非同帧」；修=decode::
  raw_orientation_of 单一 EXIF 访问器+三消费点收编+缓存盐 2→3；**存量坐标迁
  移（用户拍板=加载时自动旋转）**：新字段 coord_era（0=传感器帧/1=显示帧；
  字段级 serde default 压过容器级=否则旧文件全读成已迁移，专测钉；不复用
  version=它在四个主路径位点被有意移植，单整数装不下两个生命周期）；六个读
  文件载入点走单一 LoadMigration 漏斗，模型 JSON/HTTP 体/XMP 三个出生点边界
  盖当代章；orient_point 像素孪生逐州推导+61MP 旋转峰值实测 1384MB；栅格蒙
  版不迁移改披露。**⚠ 前向：v0.29 exe 拒读 v0.30 保存的任何 recipe.json**
  （coord_era 顶层字段+deny_unknown_fields 既定姿态，app 内部数据无 XMP 影
  响）。**计划外根修②联合梯子定稿（R23 provisional 债销账）**：六对 EXIF 确
  认同帧真实对实测（方向修后重跑）——用户口中「瞎搞」的天体合成对联合读数
  0.141 在旧线 0.25 下无警示还报 0.578 置信；新常数 JOINT_FAR_ERR=0.10（夹
  逼：上界=A1 必触/下界=canyon-gold 0.093 策略性拒绝不受指控留 8% 余量=下一
  真实失败对到手第一个复议的数）+SLOPE=7.5（(1−floor)/FAR 绑定保留，真实对
  从顶端独立佐证同向）+真实锚点进**编译期** const assert；**同帧警示置信帽
  0.5 新增**（40 组纯裁剪自配对实验：框取差异凭空造 sat±23/EV±0.45、最大残
  差 0.0854→conf 0.488=帽值出处；是帽不是地板=LR 导出裁剪是工具提示自邀工作
  流）；域依赖发现（联合读数随求解域变，梯子按 CLI 域=故障报告域标定）入挂
  具 doc。M-D 处置：denoise exit-0 **四组合实跑否证不可复现**（补合约测试
  钉性质）；光栅门文案去内部符号（测试钉「必含文件名必不含 ::」）；AI
  Denoise 补 segment 同族能力探针+zh 逐字同形。**素材自采四条全录**（零付费
  零写库）：M-A 六对（stem 同名 116 只有 54 真同帧=EXIF 时间戳教训）；M-B
  160 XMP 取证含载荷性阴性结果（Add/Subtract 全顶层、组内组件 0/428——B 批
  设计据此，~~Defringe 全库 0 待用户 LR 补做~~ **← 2026-08-18 已补做：
  `_DSC9597.xmp: DefringePurpleAmount="10"`，见「台账勘误（2026-08-19，
  R27 Batch-1a）」条①**）；M-C=整个 Raw 树恰 147 对，15
  张小样已跑（数字见 scratchpad mc-eval-15-sample）；M-D 两臂文案表在案。
  **登记不修**（五条，2026-08-19 R27 Batch-1a 逐条结算）：
  ①LOW-2 Analyze 继承 refuse_unresolved_strip=与 Ctrl+S 一致化
  设计 **← 用户 2026-08-19 裁决＝终态**；
  ②~~N3 崩溃窗孤儿 meta 行不可达已滤~~ **← 关闭为纵深防御注记**：该登记的正文
  自陈「不可达已滤」——孤儿 meta 行在渲染前已被过滤掉，登记的是「万一过滤失效」
  的第二道防线，不是待办；保留代码里的过滤，台账不再计为未决项；
  ③~~straighten≠0 时 crop 迁移一阶近似~~ **← R27 Batch-1a 已修（L-16c）**：读码
  定性后发现只有一半为真——裁剪的参照系是 `inscribed_dims` 的内接帧，而该函数
  对 `w↔h` 严格交换等变，加上旋转互相可交换，**四个纯旋转态本来就精确**；真正
  的残差是四个**镜像**态下 `straighten_deg` 没有取反（`rot(deg)∘镜像 ==
  镜像∘rot(−deg)`），已按椭圆 `angle` 的同一条规则修；八态全精确，2.5°/−44°
  往返测试 + 变异验证在案；
  ④~~web「RAW 不可读仍回存」窄残留实际不可达~~ **← 关闭为纵深防御注记**：同②，
  正文自陈「实际不可达」；路由到不了那条臂，登记的是防线不是缺口；
  ⑤LR sidecar 对迁移坐标的对齐待真实验证 **← 仍开**（L-16e，五条里唯一有真实
  残余风险的一条，且 v0.32.0 的 `(32,20)` 位移与 `k=1.032` 帧仿射让它更值得做：
  在 Autoshop 里改一张照片 → 写 sidecar → 用 Lightroom 打开目验蒙版/裁剪落点）。
  **后续登记**：底部变体条物理合并（共享属主已铺=删除+重钉小活，待
  用户裁布局）；SF9-B 版本捕获像素；M-C 全量 147 重基线待小样判读后拍板；
  ~~B2-B5 待 Defringe~~ **← Defringe 素材 2026-08-18 已交（`_DSC9597.xmp`），
  B2-B5 已于 R25 v0.31.0 / R26 v0.32.0 全部落地**。发版：feat b42c1d9，tag v0.30.0→b42c1d9，assets 回下载字节验证=本地逐位相等：cli 34b5e7b5…=28,927,209 B / gui 4a12a965…=38,331,807 B。——
  三轮清账（R22 v0.28.0 / R23 v0.29.0 / R24 v0.30.0）至此收官，下轮开工先读
  本条。

- **v0.29.0 RELEASED（2026-08-17，第二十三轮=17 条反馈三轮清账之 R23：AI 大
  修）** — 锁定计划之 R23 全量落地，六个实现提交+一个轮末复审修复提交
  （f0c7804→32cb378，cbae6bc 起累计 ~+8300/−1300），测试 467→518 lib/8 CLI
  /89→99 GUI/2 合约双配置，clippy 0，audit_i18n 0。用户条目闭环：**#12**（LLM
  知道并用全部工具：`src/advisor/catalogue.rs` 控件注册表 33 recipe+22 local，
  E0027 穷举解构双钩强制——加字段不登记=编译错；schema/提示词目录/eval 标尺/
  style 键四消费者同源派生，线上 schema 字节不变钉；HSL 7/9 元素修复不再弃
  付费提案；WB 绝对/相对成对+as-shot 实值锚点；lens 三值语义 null=无意见/
  数值=采纳/0=清零 + 局部 Hue(±100=±30°)/带符号 Sharpness 进 schema）；
  **#6**（参考库=风格索引+图片参考「都要」：EffectiveIndex 单一读取器三壳收
  编、AI 面板建库/进度/状态行、近邻披露 ≤4 stems、图片参考开关默认关+成本披
  露；披露块移 judge 后=采纳修订不再吞掉不可用注记）；**#5**（AI 太保守：
  GradeStrength 0..1 六闸同轴——提示词三档/temper 膝点缩放/护栏单边放开/
  judge 目标 78/82/88/风格参考 ceiling↔floor/无 key 启发式；CALIBRATED=0.5
  =护栏**数字**逐位复现点（旧版克制**措辞**属 ≤0.4 档，无单一档位完整复现旧
  请求——轮末复审 MED-1 重述）；DEFAULT=0.65=用户拍板⑦，batch 无旗=沿用默认
  也是拍板⑦的含义；白点保护 floor 移 soft-cap 后=任意强度不再被削，0.5+ 位
  级不变）；**#13**（思考模式：think 信封 {scene,tool_plan,intended_look,
  recipe,self_critique,pixel_tool_suggestions}，think:false 请求字节不变钉；
  多轮 judge 收敛档内 1/2/3 轮上限=think**或**Committed 析取（三 tooltip 成
  本披露 17 调用/10 带图同析取——轮末复审 MED-2）；batch/eval 结构性恒关）；
  **#3/#16**（反推大修：诚实性批=置信族一次标定两端具名+ratio 拒绝披露+终局
  do-no-harm 重置升状态行+分区置信从被采纳 zone 读；入口放开=同帧任意显影可
  选（拍板⑤），比例失配警示点名裁剪优先（轮末复审 LOW-3）；联合值域桶 4 亮
  度带×2 色度类=第二只独立读数——报告+置信帽+管线末端 fail-open 否决**仅此三
  角色**（实测：不能当阶段闸，正确修正会让最差桶变差）；「反推瞎搞」案首次
  可见=look_err 自评置信 0.52 被联合梯子压到 0.25 地板；勾选式深度反推（拍
  板⑥）=persist 前判+至多一次引导重试，hint 只做闭列表动作选择器（Zoned/
  Saturation±10 常量），采纳仅当复判 ≥；**v0.26.2「评分只作参考」定案在
  深度路径上修订**：默认路径原样保留 v0.26.2 时序，勾选深度=用户显式授权
  判在存前）。轮末独立 Opus 全量对抗复审 SHIP-WITH-FIXES（3M/6L/4N 零正确
  性/安全/契约缺陷）→ 七根因一次修复批 32cb378（0.5 双轴重述×6 处+eval 注
  释/成本披露析取+zh 70% 语义纠正/深度候选报告所有权=fit::compose_report 单
  一注记派生路径+rescore 删除+CLI 同根缺陷同修+DeepFitOutcome 四态+FitReview
  Plan 纯函数钉两次付费上限+被拒 zoned 候选栅格清理/同帧 doc 与裁剪归因/
  hint 子串 less·over 移除（"lower"⊂"flower" 残留披露在码）/FIT_JOINT=off
  四消费者枚举/白点 floor）。**登记的未验证项（诚实边界）**：联合梯子=合成
  夹具标定 provisional，待 M-A 真实复现对复核；LocalHue/LocalSharpness 的
  Lightroom 数值尺度 unverified（V2_PLAN §7 item 10 附关闭探针）；think 信
  封字段生成顺序 unverified（serde_json 无 preserve_order，properties 恒字
  母序，env 探针 AUTOSHOP_THINK_PROBE_KEY 在册）；**eval 分数跨 R23 不可比**
  （提示词全档重写），147 张对照集重基线待 M-C——**已由 R27 落地的新基线作废
  （L-31 结清，2026-08-19）**：`mc-eval-147-r27.txt`（HEAD f43dd85、中转+并行、
  零回退）为唯一参照，详见 M-C 行。**行为变更（发版说明）**：
  带蒙版新 recipe（hue/sharpness 非零）被旧 exe 硬拒（deny_unknown_fields
  前向语义，无蒙版字节不变）；R21 已删版本登记簿结构臂对新 schema 失配=一次
  性以**新号**重新保档（hwm 臂不动=同号永不复活，合约测试 2/2 绿）；深度反
  推与高强度（>70%）各自独立把视觉复查上限提到 3 轮=worst 17 调用。素材欠
  （用户）：M-A 反推复现对/M-C 147 张集路径/M-D #2 报错原文；R24（数据模型
  +LR 差距 B 组）待 M-B sidecar 族。发版：feat 6910449，tag v0.29.0→6910449，
  assets 回下载字节验证=本地逐位相等：cli 9d97e641…=28,817,006 B / gui
  68365630…=38,155,864 B。—— 下轮开工先读本条+第二十二至二十四轮锁定计划段。

- **v0.28.0 RELEASED（2026-08-17，第二十二轮=用户 17 条反馈三轮清账之 R22：
  缺陷+UI）** — 锁定计划（上方第二十二至二十四轮段）之 R22 全量落地，七个实
  现提交+一个复审修复提交（3f13bd8→52c2400），33+ 文件，测试 449→467 lib
  /7→8 CLI/73→89 GUI/2 合约。用户条目闭环：**#2**（蒙版精修报错=load_image
  「仅光栅」前提无机制——门内拒 RAW+`render::source_pixels` 统一分派收编 5
  处手抄+巡逻测试改逐调用点 why 标记；masks.rs 走真显影双绑定 guide=
  active_source_path/claim=src_path；聚合预算预检（复审 HIGH-1：预检单栅格
  vs 加载器聚合两把量尺，>53.7Mpx 双精修炸导出——同源判据修）；分割 sidecar
  缺失按钮置灰披露）；**#17**（缩略图恒定槽位+保比内嵌，方向链本就正确；
  STRIP_THUMB_H 解遮蔽）；**#9**（画笔滑杆进蒙版会话行绑同一 self.brush+
  孤儿会话修）；**#15a/#10**（蒙版 clarity/dehaze/texture 上引擎：dehaze
  拆分 golden 钉、加权 unsharp 严格等价推导、融合通道保持不拆（部分权重
  渲染兼容裁决）、engine_active 两消费点同扩；面板 LR 三组 Tone/Detail/
  Color、Temp/Tint 实时开尔文换算、color_gains 披露+清除）；**M6a**（XMP
  导出侧五类损失写侧单一来源穿 GUI/serve/CLI）；**#4/#14**（panels/ai.rs
  一级 AI 面板三子区，editable 门/R19 宽度算术/can_fit 门外三条警戒线变异
  钉；FIELD_W_MAX=420 尺寸令牌+prompt_field 原语聚焦放宽；分组弱标题+画笔
  共用小节）；**#11**（导出分裂按钮+目标三态设置 Prefs+绝对路径提示+批量
  共读；Download… 退役 9 词条）；**SF8**（XMP 一键导出到照片旁：原子
  create_new 永不静默覆盖+二次确认——「照片库只读」不变量的唯一文档化例
  外，README 同步）；打磨批（● 指示器谓词对齐字段集、STYLE_STRENGTH_
  DEFAULT 单一常量、四 Full-res 限定词）。**Opus 5 全量 diff 对抗复审
  SHIP-WITH-FIXES：1H/2M/5L/5N 全处置**（10 修入 52c2400；3 有据不修：
  BYTES_PER_PIXEL=24 既往偏低~17% 本轮无新峰值登记 R23、M6a stderr 时序
  与既有 merge note 一致、id_salt 迁移折叠态一次性重置即本条披露）；13 项
  契约检查全绿（R21 登记簿零触碰、R20 三定案、D13 键位、变体不混合 busy
  门不可达）。**发版披露**：①audit_i18n 在 HEAD cbb9900 本就红（v0.26.2
  的 JUDGE_UNCHANGED 缺 zh 对，与该轮台账「audit_i18n 0」矛盾）——已补对；
  ②带蒙版三项的旧 develop 重渲染新观感（用户 2026-08-16 批准）+judge 分
  数基线随像素移动；③clarity-only 位图蒙版开始加载栅格；④「803 子集」精
  确口径=SC 单面字形数，五字体 cmap 并集 2396 码点；⑤保存侧 MaskLoss 红
  toast 频率待用户实测反馈；⑥.gitattributes 钉 *.py=LF（此前干净克隆
  denoise 测试会红=「全量绿」不可复现，已修）；⑦L4 常量修正登记 R23。
  门：**467 lib+8 CLI+89 GUI+2 合约双配置、clippy --all-targets --bins
  --features gui 0、audit_i18n 0、字体门绿**。
  发布戳：feat `82ffa61`（实现链 3f13bd8→52c2400 共 8 提交），tag
  v0.28.0 → `82ffa61`；assets 回下载字节验证与本地构建精确相等：cli
  `796687fe…` = 28 279 766 B、gui `542e1ee4…` = 37 500 672 B（--version
  自证 0.28.0；下载路径首遇 GitHub 503 两次，直链复验通过）。

- **v0.27.0 RELEASED（2026-08-14，第二十一轮=版本快照删除复活根修）** —
  用户报"存档（快照版本读取）删不掉：点删除显示已删，重开 AS 还在"。
  **确诊（repro 测试第一方复现，一类根因三条复活臂）**：①backup gate 的
  "已保存过"去重判据就是**可删除的版本文件自身**（store 半边比 incoming、
  xmp 半边比最新 `v<n>.<stem>.xmp` 见证）——删版本=删判据，下一次程序性写
  入（AI 分析持久化/反推/批量粘贴/CLI/serve）原样重存；②`claim_version=
  max+1` **立即回收刚删的号**，GUI 版本行只显示裸 `v{n}` 无时间戳，重建
  快照与被删者不可区分；③（Opus 反审 HIGH-1）legacy `./out` 迁移只查
  `to.exists()` 而 legacy 源按设计保留——删掉中心 v<N> 后**每次打开照片
  迁移器把它原样再发布**（最贴合"重启才复活"的观察）。**修（用户拍板
  "双修：号不复用+删除墓碑"，一个机制堵三臂）**：develop 目录永久登记簿
  `.deleted-versions.json`（`DeletedVersions`：hwm 号高水位+每删一条指纹
  记录 recipe raw 字节 fnv64/结构 hash（剥 `v<n>.` 冻结前缀的紧凑重序列
  化）/**光栅字节 hash**（结构相等不足为凭——认领名可被释放重铸、领养原地
  替换；命中结构才懒读光栅）/xmp 字节 hash）——claim 地板计入 hwm 号永不
  再发（登记簿损坏→大声拒绝认领而非静默复用）；gate 两半各加"已弃置"判
  据（**契约新例外**：用户删过的内容答 Ok(None)，调用方覆写后无存留副本
  ——删除=明确弃置，指纹只为去重与烧号，非恢复副本，删除照旧释放空间；显
  式「＋ Save as version」不受限）；删除事务=标记→登记（源在时取指纹）→
  清扫→消标记，恢复路径先补登记（半清扫只烧号）；迁移守卫跳过已删号。
  **Opus 5 反审 2 HIGH/3 MED/3 LOW/2 NIT 全处置**：HIGH-1 迁移臂修+钉；
  HIGH-2（压制关闸=数据不留底）按用户已批语义保留、机械缺陷修（指纹含光
  栅字节，无悬空副本误判）；MED-4 无界读→capped+流式；MED-5 地板可擦/不
  追溯→单文件 hwm 领养随行（修复前已删的号无记录不可追溯，披露）；LOW-6
  原子性→单条目单次 durable 写；LOW-7 两豁免补钉；LOW-8 无限累积→指纹化；
  NIT-9 光栅假设→字节入指纹+双向测试（字节不同→照常保档新号）；NIT-10 幻
  影烧号维持（反审自证非攻击面）。合约测试 tests/repro_deleted_version_
  resurrection.rs（修前同 loop 2/2 命中复活为对照）+单测四处扩展（被杀删
  除烧号、光栅双向、迁移守卫、clear 幸存+显式认领豁免）。门：**449 lib
  +7 CLI+73 GUI+2 合约、clippy --all-targets 0**。
  发布戳：feat `2ae315c`（6 文件 +576/−21），tag v0.27.0 → `2ae315c`；
  assets 回下载字节验证与本地构建精确相等：cli `420ef3d7…` =
  28 294 853 B、gui `37e9e9cc…` = 37 462 647 B。
  反推 AI 打分）** — 用户令"①加强 AI 检测（AI 出 XMP 那个）效果；②反推加
  LLM-as-a-judge 式 AI 评审、做成按钮可选；③路线图全清、debug 全过、全量
  文档、发 v0.26.2"。**共同最上游缺口=AI 产出从无像素级回看**（提案盲出、
  收验只看数据、反推只报统计残差）→ 一个共用**视觉评审角色**
  `advisor::judge`（新文件）：双图（reference/candidate）入、严格 schema
  `{score,decision,critique,hint}` 出，store:false、评分钳 0..100、非有限
  拒绝、critique/hint 1024B 有界脱敏；`JudgeImages` 具名字段防形参对调
  （Opus S6 类型级修法）。**①检测闭环**（pipeline `produce_recipe`）：验证
  后的提案经 `develop_preview` 渲染 → 评审打分；revise+hint 时**引导修订一
  轮**（`visual_review_round` 纯函数：再评分 ≥ 原分才采纳=do-no-harm、同
  设置修订短路不再付费、任何失败保留已评配方），六个分支全走 rationale
  新键（JUDGE_SCORE/ADOPTED/UNCHANGED/KEPT/ROUND_FAILED/REJUDGE_FAILED/
  UNAVAILABLE，en/zh 成对）；证据升级 `hist_summary` 加亮度五分位+RGB 通道
  均值（提案与收验双方首次拿到锚点级+偏色证据）。**评审位于外观变异链末
  端**（Opus M1+S1+S2 同源折入：风格蒸馏上移到评审前、WB 锚取自唯一提升
  快照——temperature_k 是相对锚点的绝对开尔文，无锚克隆把钨丝灯校正渲染
  成恒等；refine 位图蒙版经同一 carry_over 挂入渲染克隆），渲染克隆按
  refine-strip 规则不带 base_curve/lens（嵌入预览像素已含）。**判费=显式
  调用方决策**（Opus M2/M3）：`judge: bool` 参数——GUI/web/CLI analyze/
  auto=true（工具提示+CLI 帮助披露加价），batch/eval=false（500 张批处理
  不得静默翻倍开销；eval 量的是裸提案）。**②反推 AI 打分**：GUI 复选框
  「AI review / AI 打分」（Prefs 持久化默认关=付费自选；置于 can_fit 门外
  ——偏好不是动词，Opus N1）+ CLI `match --ai-judge`；评审渲染域=各自求解
  域（GUI=源中性×复合配方；CLI=印章前嵌入预览域，后盖章会双重应用基线
  曲线）、持久化锁外执行、任何失败降级为状态注记（FitNote::AiReview/
  AiReviewFailed）；无取消与反推本身同构、工具提示如实披露（Opus S5 定
  案）。其余折入：S3=同设置修订不得谎称"已采纳"（strip 掉 prose 比较）+
  提示来源改"automated reviewer"（不再冒名 verifier、untrusted 栅栏保留）；
  S4=评审提示词声明渲染不含裁剪/矫正只判影调色彩；N2=提示前缀缩短（1024B
  预算全留给评审自己的话）；N4=全零直方图分位报 0 与均值一致；N5=超 16KiB
  配方上下文整体省略不截断（截断=喂坏 JSON）；N6=fit 侧不带 decision 有
  据（分数即结论，阈值钉在提示词里）。**字形纪律**：新 zh 词条全部用成品
  字体 cmap 并集逐字验证（付/短/简/英/买 五次撞门全改写），子集免再生成，
  803/803。门：**449 lib（+8：judge 桩 4/闭环纯函数 3/hist 分位 1）+7 CLI
  +73 GUI、clippy 0、audit_i18n 0、字体覆盖过**。Opus 5 反审 **SHIP-WITH-
  FIXES：3 must-fix + 7 should-fix + 6 nit 全折入**（N3=超长批评触发整段
  英文回退为既定契约内行为，不改）。**在册可行动项=0，登记披露=0**。
  发布戳：feat `06fb8c1`（21 文件 +1237/−56），tag v0.26.2 → `06fb8c1`；
  assets 回下载字节验证与本地构建精确相等：cli `031e7466…` =
  28 290 079 B、gui `f46bb0e7…` = 37 457 908 B。

- **v0.26.1 RELEASED（2026-08-13，第十九轮=登记项实修清零 + Generate 按钮
  一行化）** — 用户令"把所有问题直接清完（不再披露/在册）+ AI 生成按钮改
  一行高"。①**分区拒试段清零**：SKIP 线与接受地板**拆分**（fit_zoned.rs
  `ZONE_SKIP_ERR=0.012`=观测已匹配域上限、`ZONE_MATCHED_ERR=0.02`=有效修
  正落点上界），(0.012,0.02] 恢复尝试由 `zone_accepts` 裁决；两把绝对量
  尺**均带 0.25-stop EV 伴随**（线性光域在暗区一个绝对数能藏近一档亮度：
  sRGB 0.12 vs 0.173 记 0.012 却差 0.9 EV——skip 不得判匹配、落点也不得
  算 matched）；skip 判定纯函数化 `zone_skips` 并加拆分钉（0.0121 必须尝
  试）；ZONE_DROPPED 话术自洽化（"或 ≤ {floor} 同时增益 ≥ {gain}%"，en/zh
  成对，audit 0，zh 用字避开子集外字形——首版"且/改善"两字撞字体覆盖门，
  改"同时增益"后过）。真实对复跑：sky 0.076→0.007 attach、land 0.009 报
  already-matches，行为不变。②**复合域锚点五对全测**（此前仅嵌入预览域）：
  r16 复现测试永久打印复合域误预测（`neutral_gate_misprediction` 设
  pub(crate)）——fall-back 侧 9608×re2 0.024 / 9621×re 0.043 / ×re2
  0.033 / ×re4 0.036，keep 侧 ×re3 **0.0131**（1.15× 余量）；**五对裁决
  两域全部同侧**（haze 夹具为配方渲染对、无 RAW、复合域不存在——非缺口）。
  ③**披露措辞全部定案化**：盲带→"exempt by design+夹具巡逻"、<0.03 弃权
  →"by design（0.015 处实测噪声定罪特征）"、均匀>1.75× 回退→"无条件 by
  design（低误预测不得推翻份额门：错位恰伪造低读数）+synthetic-only 限定
  语恢复"、量化纹波→20:1 平铺陈述；grep 全库 disclosed/Known residual/
  Residual 清零（仅存 rationale channel 语义用词）。④**✨ Generate 按钮
  一行化**（retouch.rs）：根因=文本框预留固定 130px 按钮实需 **131px**
  （Button 样式+padding 实测；zh 只要 103——首版注释把因果写反，Opus 无头
  探针纠正），文字在按钮内折成两行；修=按本地化标签 galley 实测宽度预留
  +TextEdit 边距**由本行常量持有并交给控件**（第一版漏掉 8px 边距、配合
  Extend 造成侧栏每帧+8px 跑飞——Opus 探针抓出 BLOCK 后重做）+Extend 禁内
  部折行；新增无头回归测试双语×3帧钉"面板宽稳定+按钮单行高"，**两个故障
  模式变异注入均必红**（[427,435,443] 跑飞 / 40vs26 两行）。⑤Lang 补
  Debug derive（测试需要）。Opus 5 反审：GUI 判 **BLOCK**（探针实证）+
  fit/zone 判 SHIP-WITH-FIXES——全部折入（A1 话术自洽/A2 陈例 0.9EV 重算
  /拆分钉/OR 臂 EV 伴随/B2 第五对补测/B3 1.15× 具名/C 限定语）。门：
  **441 lib+7 CLI+73 GUI、clippy 0（cargo clean 全新跑）、audit_i18n 0、
  字体覆盖过、env 复现过**。**在册可行动项=0，登记披露=0**。tag
  v0.26.1 → `138f1cf`；assets 回下载字节验证与本地构建精确相等：cli
  `3a9def2b…` = 28 212 367 B、gui `8d07ab9b…` = 37 382 759 B。

- **v0.26.0 RELEASED（2026-08-12，第十八轮=在册清零·分区门量纲化+锚点还债）**
  — tag v0.26.0 → `72a15ac`（R17=`50ad2ae` 同版）；assets 回下载字节验证与本地
  构建精确相等：cli `abd8f323…` = 28 213 103 B、gui `8c876328…` =
  37 381 924 B。R17（下条）与 R18 同版发布。用户令"解决所有问题"，在册与
  登记项全清：
  ①**分区 ≥50% 门槛立案销案（重构落地）**：R17 全局修复后真实对复跑
  （CLI match --zoned）格局反转——sky 分区 0.076→0.007 **attach**（增益
  [0.92,1.02,1.07]+sat10，旧"25% 改善被弃"之案随全局修复消解），land
  0.009 本就已匹配却被拟合致回退（→0.029）再报误导性 "dropped: needs
  ≤50%"。根因=**相对门无绝对量纲**（按 golden-sky 0.507→0.015 级大修标
  定）。重构（fit_zoned.rs）：`ZONE_MATCHED_ERR=0.02` 绝对地板（锚点：有
  效修正落点 0.007-0.015 / 已匹配域 0.009-0.012 / 需修域 0.076/0.507）
  +**EV 伴随条件** `ZONE_MATCHED_EV=0.25`（zone_err 在线性光域，绝对地板
  在暗区意味 >1 EV——Opus 反例 sRGB 0.12 vs 0.20 记 0.020 却差 1.3 EV，
  必须拟合不得跳过）+已匹配即诚实新话术 "already matches — no correction
  needed"（新 rationale 键，zh 成对，audit_i18n 0）+接受谓词纯函数化
  `zone_accepts`（减半 OR（落地板 AND ≥20% 增益——堵 0.0201→0.0200 髮丝
  移动白买 +0.02 漂移预算））+ZONE_DROPPED 模板补 "or ≤ {floor}
  outright"（floor 印 :.3 防 0.020→"≤0.02" 自相矛盾）。真实对复跑确认：
  sky attach、land 报 already-matches、EV 伴随零误伤。②**误预测门标定债
  还清**：存档 4 真实对实测（嵌入预览域）——keep 侧真实锚点
  _DSC9621×reimagine-3 **E=0.0050**（份额比 1.12，天花板下方 3.0×），
  fall-back 侧 ×reimagine 0.074（2.65×）/×reimagine-4 **0.034（1.51×，
  份额门盲区、误预测门唯一防线的真实实例）**/×reimagine-2 0.024
  （1.92×）；天花板 0.015 两翼皆有真实对。③**1.75× 份额门取舍定案=保留**
  （互补非冗余：份额门无对齐依赖，兜误预测门 fail-open 的错位场景；均匀
  >1.75× 膨胀误回退为已接受残余，仅合成可构造、两真实回退解均优于门控
  臂）。④**盲带哨兵夹具**：cc∈[0.03,0.05)×wc∈[0.04,0.09) 三断言巡逻
  （带内 174° 豁免=0 / after 过 0.09 计入 / before 过 0.05 计入；Opus 验
  算边距 ≥0.005 无 f32 脆弱性）。⑤advisor 存根偶发销案：R17-R18 全套跑
  ≥6 次未再现（再现则重开）。**Opus 5 反审 SHIP-WITH-FIXES，2 must-fix
  +2 should-fix 全折入**：i18n 门红（ZONE_DROPPED en 陈文+新键无 zh 对）
  →字对齐+补对、复审后 audit 0 且字形全在 802 子集免重生成；线性光量纲
  →EV 伴随条件；OR 臂增益下限；相对臂变异洞→加 (0.500,0.200) 钉。披露
  折入：~~matched 地板 (0.012,0.02] 拒试段~~、~~GUI 复合域锚点未测段~~
  （两者均已在 **R19/v0.26.1 实修实测清零**，见首条）、两门互补段
  （fit.rs/ARCHITECTURE 同步）。门：**440 lib（+3：谓词域/已匹配端
  到端/盲带巡逻）+7 CLI+72 GUI、clippy 0、audit_i18n 0、env 复现过**。
  **在册可行动项=0**。

- **R17（2026-08-12，第十七轮=反推灰纱根修·证据识别自警；feat `50ad2ae`，
  随 v0.26.0 发布）** —
  用户三报同对反推异常（_DSC9608 × reimagine-2）：v0.25.0 复合解渲染**灰闷**
  （第一手复现：CLI apply 全尺渲染+4×4 格网量化——中间调比目标暗 5-16/255、
  地平带 −13、阴影抬 +15），而标量 err 0.095→0.044 自称胜利。**溯源链**（全
  程第一手，消融渲染定责）：饱和度对亮度零影响（fitted==no_sat 逐行）、滑杆
  独渲比承诺亮、**残差曲线是压暗执行者**——它忠实执行了坏的证据映射；坏在
  **灰证据识别失效**：目标把 base 灰类 24% 成员（淡色天空，亮度 q50≈197/255）
  重上色成浓蓝，两侧"灰"换血（Jaccard 0.74），份额比 1.29× 溜过旧 1.75× 门
  → base 亮灰秩错配 target 暗灰秩（灰 q50 映射 151→109），全帧拖暗。
  **判别量三代实测淘汰**：成员 Jaccard（误伤良性嵌套）→ CDF 污染度（haze
  0.128 > 毒对 0.040，反序）→ 定稿**误预测门 `neutral_gate_misprediction`**
  （机制本体：门控证据映射对"共享类"=两侧同位皆灰人群自身可观测配对的预测
  误差；公式与生产 tone_map 逐步一致；_DSC9608=0.021、haze=0.126 均回退，
  canyon/identity≈0、合成均匀膨胀 <0.0075 保门；阈 0.015）。**haze 夹具连锁
  真相**：其门控证据一直是坏的（蓝罩把 clean 侧暗灰染出类外，目标侧多 6740
  暗成员），旧 9 点稀疏残差采样把坏映射糊过去——R17 **曲线加密**（13 点 LUT
  输出域均匀反演 partition_point；结间隙 38-u8→~21-u8，线性弦下垂 ~10/255
  消除）让求解忠实 → 门控臂当场塌缩（0.0892→0.0892 复位），回退臂
  0.0892→**0.0229**：两项修改互为表里。**转轮普查穿零豁免**：haze 修正增强
  后 4.5% "两端色度均不可见"像素（before<0.05 全体、after≤0.082、45°→120°
  份额平坦=蓝罩反相穿零）被计 ≥75° 吃光 0.1× 余量 → `rehued_share` 增两端
  可见性豁免（ROT_VISIBLE_BEFORE 0.05 / AFTER 0.09），H17（0.035→0.34）与
  canyon（before≥0.05）不受影响，0.1× 余量钉**结构性复活**（非放宽）。
  **真实对终态**：复合解 0.0947→**0.0267**（sat 52.6 脱顶格），渲染亮度分位
  [72,98,123,152,183] vs 目标 [74,100,126,152,182]（mean 1.6/255；已入
  env 测试为契约断言 ≤6/255），目验灰纱消失（三连对比图已发用户）。跨解器：
  中性源解 0.0252、复合刚度 +6%——env 断言改"相对 1.10×+1e-3（对退役臂的
  诊断）+分位贴合（真契约）"，配方 JSON 落盘臂（AUTOSHOP_FIT_REPRO_OUT）
  移到断言前。**Opus 5 反审 SHIP-WITH-FIXES，4 must-fix 全折入**：①const
  锚点表 haze 侧写反已改正（haze=0.126 在回退侧）；②**标定债如实记档**：最
  近毒锚 0.021 仅 1.4× 于阈值、keep 侧无真实对锚点（只有合成夹具），待真实
  门控对捕获；③**位置对应假设显式记档**：失效方向=开门（错位→指标滑向 0→
  检测器空转，老门仍在前排），网格差 >2% 显式 INFINITY 回退；④共享类门槛
  具名化 `enough_evidence`（5%+512，与两侧同一标准）。should-fix 折入：线
  性弦措辞更正（残差曲线走 curve_lut 分段线性、非单调三次）、平台采样语义
  与量化纹波代价披露、ROT_VISIBLE_AFTER 0.10→0.09 并披露盲带
  （cc∈[0.03,0.05)×wc∈[0.04,0.09) 无夹具巡逻）、haze 端到端钉收紧
  0.6→0.35×、is_neutralish 文档改"仅色调证据"、limiter 陈数标注历史（fit.rs
  与 ARCHITECTURE 同步）。**有据不做/在册**：①~~分区 ≥50% 门槛立案待查~~
  （→**R18 重构销案**，见首条：绝对地板+EV 伴随+增益下限；真实对复跑 sky
  已 attach、land 报 already-matches）；②"共享类当第三臂证据"（Opus 提议）
  有据不采：共享类按定义丢失迁出带亮度域（本对将零亮天证据）且继承对齐依
  赖，两个真实坏对全量回退实测更优；③~~1.75× 份额门与误预测门的关系登记为
  后续取舍~~（→**R18 定案=保留两门**，互补实证见首条）；④对称换血为指标结
  构盲点，已在 doc 披露。**历史注**：2026-07-09 批次 #2-A 所记 tone_cdf_pair
  两条件与旋转普查"两端可见 chroma≥0.04"自本轮起被三条件+可见性豁免取代
  （历史条目保持原文）。门：**437 lib（+5：毒对蒸馏/良性膨胀/canyon 匹配/
  haze 回退钉/结密度钉）+ 7 CLI + 72 GUI、clippy 0、env 复现全绿**。
  ~~未发版~~（→已随 **v0.26.0** 与 R18 同版发布，见首条）。

- **v0.25.0 RELEASED（2026-08-12，第十六轮=已知项全清）** — tag v0.25.0 →
  4131bbe；assets 字节验证与本地构建精确相等：cli `297a9e3b…` =
  28 208 038 B、gui `3528f5ce…` = 37 376 913 B。①**标定复合进求解闭环**
  （R16 主项）：`fit_recipe_from`/`fit_recipe_zoned_from` + 
  `pipeline::calibration_recipe`——recipe 从标定基起步，闭环候选渲染全部=画布
  单程 user(base(x))；滑杆在用户域解（其输入即基曲线输出）、残差曲线经基 LUT
  重定域（空基跳过重定域保 default 包装 bit-for-bit）；退化拒绝与 do-no-harm
  终端回退均落**标定观感**（err_after=err_before 恒等式）；v0.24.0 两趟式种子
  （fit_calibration_seed/seed_from_calibration）删除，其 scale_chroma 钳位次序
  gap（饱和 fixture 实测至 ~18.7/255）构造性归零——**"报告残差==画布显影链复算"
  以 1e-6 相等入测**（几何重采样在模型外，先在既有、二阶）。真实对：复合式
  0.0947→0.0433（此数即画布真残差；v0.24.0 的 0.0378 量在画布从不渲染的域，
  跨机制不可比）。②**版本条标题/分隔线居中已治**：painter 绝对绘制（零布局
  参与，26px 桩不变式保持 0 偏移），几何测试断言 title 与卡列中心 ±1px。
  ③**零区 EV-增益错配全读证伪**（在案项了结）：矩量 EV 仅归一化增益（三通道
  同除标量、比例不变），CDF 色调解跑在已上增益的渲染上、亮度闭环吸收——非 bug。
  ④CLI match 保留内嵌预览+后盖戳契约（preview_only 免 demosaic，已验证）。
  Opus 5 复审 SHIP（域代数逐段确认），must-fix（main.rs 陈注/台账悬空指针）
  与加固（退化臂 clamp、caller contract、空基跳过、措辞软化）全数折入。门：
  432 lib + 7 CLI + 72 GUI、clippy 0。**在册可行动项=0**。备注：advisor 存根
  测 only_a_capability_status… 全套负载下一次偶发（单跑+复跑绿，未触其代码），
  再现则查；零区分割输入=中性显影（v0.23.x 历史常态，v0.24.0 曾短暂为种子
  渲染——语义分割对此不敏感，后续真实 zoned 反推可实测覆盖）。

- **v0.24.0 RELEASED（2026-08-12，第十五轮=反推暗闷根修）** — tag v0.24.0 →
  71f9783；assets 字节验证与本地构建精确相等：cli `cf4d51ff…` =
  28 280 676 B、gui `578d8c71…` = 37 450 271 B。**根因（用户 18:38 XMP 复现
  +真实对挽具双重定罪）**：GUI 反推从 RAW 中性显影起解，而 AI 靶派生自亮
  0.6~1.4EV 的相机观感——有界模型（±60 饱和上限/转色预算/滑杆量程）容量全
  烧在补基准观感上：饱和度顶死 +60、色偏曲线三闸全拒、有效天空零区被弃
  （挽具旧路径 0.0913→0.0469 与落盘 recipe 逐位吻合）。**修**（一套共享机
  制）：`pipeline::fit_calibration_seed` 源先过标定渲染一次、解在相机观感域
  跑；`stamp_fit_calibration` 同一标定盖进产物（CLI match 与 GUI worker 共
  享）；`fit_calibration`=fit 侧权威（saved-first，**全中性回落新鲜估计**——
  堵死"上一次未盖戳 fit 的空曲线毒化权威"回环，本片正中此洞）。**真实对**：
  新路径 0.0948→**0.0378**、sat 49.3 脱顶格、EV −0.45、13 点标定已盖。
  **域诚实（Opus 5 复审 HOLD→逐条闭合）**：解域与画布单程 user(base(x)) 差
  在 scale_chroma **钳位次序**（非量化！）——近灰≈量化级、高饱和×强负 EV 随
  饱和度增长（fixture 实测 6.4~18.7/255 均值，画布只会比数字**少**裁不会
  多）；双域+双基失败案全被 `the_seeded_solve_domain_matches_the_canvas_render`
  钉住；env 门控真实对挽具 `r15_seeded_fit_on_a_real_pair` 常驻。测试 428→
  431 lib（+stamp 全字段/种子开合/两域等价钉），门 431+7+72、clippy 0。附：
  egui 版本条**布局不变式**探针钉死（滚动区按 26px 声明桩相对行当前高度居
  中再下长，偏移=(行高−26)/2；v0.23.5 等距成立恰因 26−26=0）——标题/分隔线
  的 ~29px 偏心为~~已知装饰性小瑕疵~~（v0.25.0 已治：painter 绝对绘制）。
  ~~在册可行动项=1（第十六轮候选）：把标定复合进求解闭环~~——**已于 v0.25.0
  第十六轮闭合（见首条）**。复审顺手项已折入：calibration_is_neutral 复用
  vignette_active/geometry_active、fit_calibration 直接分支快照免双 demosaic、
  actions.rs Before 注释陈账修正、err_before 语义换代（=标定渲染 vs 靶，跨
  版本不可比）在此登记。

- **v0.23.5 RELEASED（2026-08-12，第十四轮=用户试用反馈三连）** — tag
  v0.23.5 → 92197c1；assets 字节验证与本地构建精确相等：cli `4d85fe60…` =
  28 271 265 B、gui `b07be635…` = 37 440 072 B。①**版本条几何根修**：用户报
  缩略图顶到面板天花板；中途版的跨轴居中被无头探针证伪——egui 先给横排置
  26px 行桩再生长内容，`Align::Center` 居中的是桩（实测顶空 37px/列底溢出
  21px）→ 改显式顶部留白 (可用高−卡列高)/2（卡列高 = THUMB_H 52 +
  item_spacing + interact_size.y 活令牌），面板高 96→104 收单一权威
  `AutoshopApp::VARIANT_STRIP_H`；develop.rs 三枚 cfg(test) 测量缝 + 无头双
  场景几何回归测试（孤 Original / 含 ✕ 行的双变体）断言上下等距，实测
  8.0/8.0；Opus 5 只读复审 SHIP，其两条覆盖建议（从 card.top 量顶距、最小
  气隙下限）同轮折入。②**AI 分析 11s 超时复发定性=本机**：Radmin VPN 网卡
  metric 被写回 1（记忆 autoshop-dns-11s 同款），冷解析 12084ms→96ms 提权
  修复；用户拍板不装防复发任务、复发时手动关 Radmin。"点击 1-2 秒弹旧诊断"
  =已保存 recipe rationale 在开照/换变体时重显（actions.rs:627/1340、
  workers.rs:1126），非新请求失败，成功分析一次即覆盖。③**默认语言**：代码
  默认本就 En（app.rs Lang::En），用户机中文来自持久化偏好 lang:Zh → 已在
  GUI 关闭时翻为 En（app.ron，零代码变更）。**流程变更（用户令 2026-08-12）：
  对抗式复审弃 Codex 改用 Opus 5 只读子代理**。门：428 lib + 7 CLI + 72 GUI
  （71+新几何测试）、clippy 0（含 GUI bin）。~~在册可行动项=1（第十五轮）：
  反推暗闷~~——**已于 v0.24.0 第十五轮闭合（见首条）**：用户重跑反推的落盘
  recipe 当场定罪，三机制中真实触发的是"中性起解烧光模型容量"这一最上游
  根因（饱和度顶格+色偏三闸+零区丢弃皆其下游症状）；do-no-harm 回退
  （fit.rs:390-394）与零区 EV-增益错配（fit_zoned.rs:146-155/467-468）本对
  未触发、保持在案。

- **v0.23.4 RELEASED（2026-08-12，补丁版=图标署名）** — tag v0.23.4 →
  495cd04；assets 字节验证与本地构建精确相等：cli `dfe0b98a…` =
  28 271 784 B、gui `4e39997d…` = 37 439 625 B。用户对 As 瓦片提出法律
  顾虑并拍板加署名：**≥64px 帧顶部加 "skymanbp's" 小字行**（同色降一档、
  Noto 500，As 下移保持平衡），48px 及以下保持纯 As（4px 字行是噪点不是
  署名——小尺寸减细节惯例）；戏仿+明确归属+异字母，进一步远离 Adobe 商标
  外观。除升版外零代码变更；嵌入经 exe 资源提取验证（禁令未破）。门照
  跑全绿：428+71+7、clippy 0、E2E 22/22 真进程（GUI 4 项由十三轮解禁补
  跑覆盖，禁令在场）。用户桌面旧图标成因=旧 exe + Windows 图标缓存，
  刷新指引写入 release notes（ie4uinit.exe -show / 重启 Explorer）。
  **分发拍板（2026-08-12，AskUserQuestion）：不做安装包，维持绿色版双
  exe**（免安装、全资产内嵌；用户本机直用构建目录，外发走 Releases 页
  Assets 直下）——未来会话勿再提案 Inno/MSI，除非分发场景变化。

- **v0.23.3 RELEASED（2026-08-12）** — tag v0.23.3 → 866357b；assets 字节
  验证与本地构建精确相等：cli `633c4796…` = 28 260 608 B、gui
  `07a1d207…` = 37 420 148 B。内容=第十三轮全量（次条）。终态 428 lib +
  71 GUI + 7 CLI、clippy 0（含 GUI bin）、i18n 九门 0、字体门 802/802、
  E2E round-13 真进程 22/22 + GUI 4 项**用户轮末解禁一次**补跑全过
  （合计 26/26，均对 v0.23.3 发布二进制；补跑含真机启动 10s 无 panic、
  主窗+F1 快捷键窗双截图、CloseMainWindow 优雅退出、prefs 沙箱落位；
  截图目验=标题栏 As 图标、顶栏留白、Library 芯片全框、空态与快捷键窗
  两处彩蛋在屏——Settings 页脚彩蛋为代码+i18n 验证，未截）。
  **轮末 GUI 启动禁令已重新恢复**；在册可行动登记项=0。

- **第十三轮：恶搞图标 + 彩蛋 + 间距 + R12-03 清账（2026-08-12，v0.23.3）**

  - **图标 `9e1e83e`**：光圈轮让位 "As" 恶搞瓦片（build.rs winresource 嵌
    `assets/autoshop.ico`、gui/main.rs 读 `icon_256.png`，零代码变更；从
    构建产物提取图标资源验证嵌入，未启动 GUI）。
  - **间距 `3987fd2`**：顶栏根因=egui side_top_panel 默认垂直内边距仅 2px
    → 顶面板显式 frame 带 SPACE_MD(6px)；Library 芯片根因=egui
    `Button::small()` 硬清零垂直 padding 且跳过 interact-size 下限
    （egui-0.29.1 button.rs:202/263，样式令牌无从救）→ 四枚芯片换全框按钮
    +Small 字号（panels/gallery.rs 本地 `chip` 构造器）。
  - **彩蛋 `cd42923`**：三处（Settings 页脚"skymanbp's AS——As 是 Autoshop
    不是 Adobe 订阅，迄今月租 $0.00"、空态"比 Photoshop 启动画面快"、快捷键
    窗页脚"没有 Creative 档位没有 Cloud 没有每月赎金"），全部 weak/small 走
    tr()，ZH_ENTRIES 标注分组；新增汉字使字体子集 790→802（re-run
    subset_gui_fonts，check OK 802/802）。
  - **R12-03 实修 `5b61687`**（在册清单自此归零）：`xmlns_conflict` 从平铺
    全文扫描改**作用域栈解析**——只在绑定实际腐蚀本文档读取处拒绝：crs:/rdf:
    名字（元素或属性）在作用域内解析到非正典 URI；其它前缀解析到正典 URI
    （设置藏进扫描器看不见的拼写）；无前缀**元素**处于绑定到正典 URI 的默认
    命名空间（无前缀属性依 XML 规范不入命名空间）。未被解析经过的声明惰性
    （嵌套孤岛重绑不再拒全文）；未声明的 crs:/rdf: 照旧放行。同改协同堵洞：
    `find_crs_description` 曾仅凭 `xmlns:crs` **属性名**认合并目标——作用域
    门放行的"未使用异绑 Description"会被选中并拼入正典意图设置 → 认定改为
    声明**值**必须正典。6 新测试 + 五路变异恰红（平铺化/属性检查失效/不弹栈
    /只看名字/无帽；变异 B 额外红了 BF 批既有测试
    a_foreign_camera_raw_prefix_refuses_the_merge_and_is_disclosed——同机理
    第四探测器，夹具 xmp.rs:4152 亲证）。
  - **Codex xhigh 只读复审闭合 `3f3df86`**（2 发现均 MEDIUM，先第一性红测
    复现再修，双变异恰红）：**R13-01** 帧按深度算术弹栈——冒名/多余闭合标签
    （`</bogus>`）提前释放孤岛异绑，旧平铺门拒绝的文档穿过作用域门（我注释
    宣称"malformed 只朝拒绝退化"被证伪）→ 弹栈需深度+元素名双匹配
    （tag_name），不匹配=帧存续（只会过拒）。**R13-02** resolve 每名字线性扫
    全部活声明、单标签声明数无界 → 16MiB 信封内可构造二次方 → 1024 帧帽换
    **活声明预算 256**（弹栈递减；超限保守拒绝+披露；真实生产者 ~10 条）。
    frames 结构化为 NsFrame。GUI/i18n/合并路径复审为阴性（插入路径自声明
    正典 xmlns:crs，异绑无从俘获）。
  - **文档漂移彻查**：ARCHITECTURE 状态行 v0.23.1→v0.23.3、测试计数
    281/3/37→428/7/71（多轮陈账）；本文件「关键架构事实」两处 gui.rs 锚点
    改指模块树真实位置（十二轮拆分后失效）；README/M1_PLAN/V2_PLAN 逐一核过
    无现状式漂移。
  - **收口**：428 lib + 71 GUI + 7 CLI 全绿；全轮 `#[test]` 等集 +8/−0
    （diff 级验证）；clippy 0（lib+bins+gui all-targets）；i18n 九门 0；字体
    802/802；每提交密钥+路径扫描 0。**E2E round-13 真进程 22/22 全过**（新
    发布二进制；xmlns 冲突警告头在作用域门下端到端幸存），GUI 4 项按禁令
    **跳过并披露**（视觉验收=用户截图）。ccm 计划第十三轮用户锁定后逐步
    执行完毕。



- **v0.23.2 RELEASED（2026-08-12）** — tag v0.23.2 → 4754999；assets 字节
  验证与本地构建精确相等：cli `47dac880…` = 28 295 525 B、gui `8a8955ee…`
  = 37 501 943 B。内容=第十二轮全量（阶段 0-5 前档 + 下方阶段 6b 九批全清
  + Codex 复审闭合）。终态 420 lib + 71 GUI + 7 CLI、clippy 0（含 GUI
  bin）、i18n 九门 0、字体门 790/790、E2E 26/26 真进程。**轮末 GUI 启动
  禁令自此恢复**（本轮解禁窗口关闭；后续开发照旧绝不启动 GUI exe）。

- **第十二轮·阶段 6b：九批 67 条全清 + 两条移交深查闭合（2026-08-12，
  用户令"按顺序推完"）**

  16 路盲扫 184 发现的确证待修 72 条中，BA 批（前条）之外的九批 67 条 +
  修复期深查 2 条，全部按最上游根因归并落地，十次提交（8e87e01..ca1bcdb）：

  - **BB 耐久 `39634c9`**（store.rs 10 条）：adoption 簇六洞一体重做——resume
    以 marker 记录的源为准（三拼写不再混代）、marker 在场期目的地无权威
    （source-wins 替换复制）、规范拼写快路径每 (root,key) 一次 marker 探测并
    续作、锁内复验预锁探测（superseded 收敛/collision 转披露臂）、枚举错传播、
    写路径在 develop 锁内探测 superseded 标记并重解析（lexical 回退会话不再
    整会话写进冻结备份）；commit 族三小修（cdir 父 fsync、staged 成员超
    16MiB 恢复帽即拒存、COMMIT 标记按契约先亡）；clear 标记消费失败必须
    Err（批 S 陷阱经失败路径复活的那半）；零字节 live=死声明（双崩窗口，
    锁内清除后还原 .bak）。7 测，M1-M7 恰红。
  - **BF xmp `d0e4381`**（4 条）：xmp_to_recipe 落实披露句承诺的 xmlns 门 +
    默认命名空间（裸 xmlns=CRS/RDF）同判冲突；曲线 li 项改共享标签扫描
    （白空拼写不再静默清曲线）；HasCrop 结构一致性入披露扫描（缺坐标/倒序
    不再静默删裁切）；属性值 \t\n\r 改字符引用（合规解析器归一化不再改名）。
    5 测，MF1-MF5 恰红。
  - **BG serve `cf97685`**（5 条）：header() 强制可打印 ASCII（em dash 句与
    本地化 IO 错误不再杀死 X-Recipe-Warning——七处动态构造收拢；控制字节
    仍剥，防拆分不变）；store::develop_revision 把在位 LR sidecar 的内容哈希
    折进 If-Match 标签（GET 贴章/POST 前置/三处回执同语，保存回执改写后
    计算）；不可读 store XMP 披露并挡内嵌包；空上传 400；Analyze/Retouch/
    Heal 的嵌套配方补 clamp（对称 DevelopReq）。2 测 +3 条 handler 级注记
    E2E 面，MG2/MG3 恰红。
  - **BH CLI 预检 `321a339`**（7 条一族）：L09#1"付费前预检"规则补全非输出面
    ——produce_recipe 顶部查分析 key（缺 key 不再烧掉已付费提案，两 UI 同
    受益）；heal 预检 mask 存在/--no-auto 语义/图像 key；reimagine/retouch
    预检 key+封闭集值+mask；apply 预检扩展名；preflight_out 写权限探针。
    附带：clippy 门自此含 `--bins --features gui`（抓出上一提交漏网的
    field_reassign）。4 测，MH1-MH4 恰红。
  - **BE 引擎 `4eca147`**（9 条归 6 改）：双 vignette 合成单 LUT 单遍（钳制
    不再吃掉互逆校正）；三级内嵌回退 Err=让位下一级（尾错列全因）；TIFF
    ICC 重探失败=硬错（假 sRGB 兜底正是该函数自定义的 bug）；Meta 尺寸改
    实际交付 preview（喂 AI 与 style 特征）；退化输入拒拟合（零方差对不再
    产被 err=0 接受的自信垃圾，FIT_DEGENERATE 双语键）；clone_stamp 补
    preflight；分割/降噪产物验收（可解码/等尺寸/位深不降——1×1 PNG 不再能
    发布为 master）；denoise_file 搬运 ICC（ProPhoto 不再被当 sRGB 读）。
    字体子集重生成 786→790。7 测，ME1-ME7 恰红。
  - **BI 生成几何 `7d8bc88`**（5 条）：call_images_edit 改按尝试尺寸重建输入
    （enum 回退不再把圆拉成椭圆；计费不变量测试仍绿）；feather 改按选区
    包围盒定标（帧规则保留为上限）；合成 alpha 随权重混合（透明底不再长
    不透明光晕）；clone 供体出界丢弃不复制边缘；strength=0 恒等提前返回。
    5 测，MI1-MI5 恰红。
  - **BD 导出 `58b18f4`**（3 条）：另存对话框扩展名跟随格式下拉（同容器
    拼写保留）；Single 导出 relook 回修画布并 dirty（导出与预览不再整会话
    分歧）；GUI panic 钩→store 根 panic.log + user32 MessageBoxW（窗口化
    构建的 panic 不再无声消失）。1 测，MD1 恰红。
  - **BC GUI `126c27f`**（19 条三族）：门/几何——clear_mask 连清烘焙灰度
    （"清除后 Apply 仍提交"闭）、Full-res 复选框撤 RAW 门（denoise/clone，
    引擎早已尊重烘焙源）、develop 面板中开窗口冻结（open_in_flight 非
    busy）、Hue/Angle 滑杆端点让出规范化奇点（拖到头不再瞬移）、活动卡
    chrome 令牌化、crop_mode 翻转重基 pan、深缩放撤 4× 帽（画布不再反缩）、
    区域重绘 from_two_pos、变换键携双 profile 开关、to_norm 亚点下限对齐
    to_screen；状态——switch_variant 先 commit 重命名、切换清 after_tex 与
    1:1 缓存、烘焙 master 解码失败落 toast（Opened 元组携带、不入缓存）、
    open_path busy 臂可见拒绝、Save-all 补 pixels_on_disk 镜像；worker——
    MasterLoaded 携带 spawn 时 edge+stamp（旧注释论证方向颠倒：完成时取戳
    是假命中不是 miss）、重绘泵门移到 update() 末尾（首批 worker 不再无泵）、
    取消后迟到成功的 ./out 孤儿删除。6 测，MC1-MC6 恰红。
  - **BJ 接缝 `1e5cf67`**（9 条）：resolve_snapshot_develop 补齐 open 路径的
    披露（warns 通道上 Batch 回执双臂，中英对）；HSL_BANDS 改导入（i18n
    审计登记随迁）；导出格式线号字面钉死；export_opts 全旋钮测试；zoom
    滑翔抽 apply_zoom_glide 接缝；快捷键排他门首个阴性测试（附阳性对照；
    变异第一次打错层被抓——工具层有自己的门）；resync 走真实换源路径
    （load_version）测试；L16-13 注记（四启动点共用已测 canvas_edge，一行
    调用面）。5 测，MJ1-MJ5 恰红（MJ4 重瞄真门）。
  - **深查闭合 `ca1bcdb`**（L08-5/L08-6）：verdict 恢复臂"歧义即拒"——同一
    回复出现两个**不同**verdict 形对象即拒（last-wins 可被回显注入翻案，
    first-wins 只是换边；逐字重复仍收）；阻塞 verify 查 finish_reason
    （length/content_filter 截断响应不再靠 serde default 凑成合法 Accept；
    缺席仍放行旧桥）。2 测，MDX1/MDX2 恰红。

  纪律与门：每批变异亲证（改回缺陷→恰红本批新测试）+ 五门（clippy 0 含
  GUI bin/等集 comm 只增/i18n 九门/字体 790/密钥+路径扫描）+ 单独提交。
  累计等集 +44/−0（384→414 lib、59→71 GUI、5→7 CLI）。**E2E 第十二轮
  26/26 真进程通过**（十一轮 18 项回归 + 本轮 8 项：heal/apply/reimagine
  预检拒绝、serve 空上传 400、xmlns 冲突警告头带 em dash 端到端幸存、GUI
  真机启动+截图+优雅关窗+沙箱 prefs）。E2E 沙箱剥离全部 AI key——任何
  回归都撞预检断言而不是计费。
  注记残留：L02-7/9/11/13 无故障注入不可变异测（机理注释在场）；BG 三条
  handler 级由 E2E 面覆盖；L16-13 一行调用面注记。

  **Codex xhigh 只读复审闭合（同日，gpt-5.6-sol --background 回执 13 发现
  =6H/6M/1L，逐条对源码亲证后 11 采纳实修 + 2 有据登记，一次提交）**：

  - **R12-08（HIGH，计费）采纳**：SSE 结构化错误臂在 2xx 后重投，而
    `read_sse_image` 的 partial 计数在错误路径被丢弃——partial 帧证明生成已
    在计费。修=计数编进错误**前缀**（"image stream error after N
    partial(s):"），既有"前缀必需=计费安全"规则自动拒绝 post-partial 重协商；
    零 partial 的即时拒绝（第八轮实测存在）保留可协商。复审对 BI 的
    input-provider 重构本体判无独立缺陷。
  - **R12-12（HIGH）采纳**：Verdict 容忍未知字段，直接解析成功时嵌套的
    不同 verdict 形对象绕过歧义拒绝——`{"decision":"accept","echo":
    {"decision":"reject"}}` 曾判干净 Accept。修=解析成功后深度受限遍历全部
    后代对象，任一解析为**不同** Verdict 即拒（逐字回显仍收）。
  - **R12-13（MEDIUM）半采纳**：`tool_calls` 从可接受终态中移除（它宣告的
    是下一步工具请求，旁边的 content 是半成品）；**缺席 finish_reason 放行
    =有据登记**（旧桥兼容，代码注释在案，复审自己也注明 admitted）。
  - **R12-01（HIGH）采纳**：adoption 围栏只有 memo 化探测，权威门（develop
    锁）只查 superseded——并发采纳开始前探测过的会话被 memo 永久放行。修=
    锁内加 `adopting-from.txt` 围栏检查（锁外续作后重试；失败即拒触碰），
    memo 降级为纯优化（文档明示）；adopt_files 的 marker 消费失败改 Err
    （clear 标记同规）。
  - **R12-02（MEDIUM）采纳**：source-wins 是覆盖不是同步——目的地独有成员
    不删，别名会话在两次尝试间清掉的 recipe 会被复活并随 supersede 冻结为
    真相。修=拷贝后同步清除源中不存在的非机械成员文件；`contentful` 的枚举
    错误不再折叠成"空"（NotFound 才是空；预锁臂披露退 lexical，锁内臂传播）。
  - **R12-04（HIGH）采纳**：GET 的 body 与 ETag 来自两次独立 sidecar 读，
    Lightroom 不持我们的锁——body A 可顶着 ETag B 发出，随后 If-Match: B
    静默覆盖 B。修=`develop_revision_of(src, &ranked)`：migrate 后**一次**
    `lightroom_sidecar` 读同时喂 body 与贴章。残留登记：POST 前置条件之后、
    write_xmp 之前的 LR 换档窗口——与 Lightroom 无共同锁，固有。
  - **R12-05（MEDIUM）采纳**：`xmp_warn` 累积后，有效 recipe.json 的两个
    返回臂与 422 终态臂都不附带——"sidecar 不可读"恰在 store 有答案时消失。
    修=全部返回形状统一附带。
  - **R12-06（HIGH）采纳**：预检族三缺口——batch 在 `produce_recipe`（付费）
    之后才 ensure_parent（修=循环前对首个已认领名跑全预检）；heal 只查
    mask 存在不查可解码（修=预检改 `open_mask_bounded` 有界解码）；
    `preflight_out` 只证目录可写不证既有目标文件可替换（修=存在即开写探测）。
  - **R12-07（HIGH）采纳**：denoise 验收只看头——截断 IDAT 过检；
    位/通道折叠使 RGB16 顶替 RGBA16（alpha 消灭）；ICC 补挂在发布**之后**，
    失败留下未标注成品顶着错误返回。修=验收**全解码**+等尺寸+通道数不降+
    位深不降，解码像素复用给 ICC 重编码，ICC 落在 staged 文件上、发布仅一次。
  - **R12-09（MEDIUM）采纳**：直通道 RGB 与 alpha 独立混合——1% 权重白色
    盖透明黑得 (3,3,3,3)，下游合成近黑边缘。修=预乘 source-over（生成侧
    不透明故除法安全；不透明底数学上退化为旧行为，正常照片零变化）。
  - **R12-10（LOW）采纳**：panic 钩不建目录不查写入结果却宣称"报告已写入"。
    修=create_dir_all+按写入结果分文案。
  - **R12-11（MEDIUM）采纳**：`xmp_arm` 在 no-op 导入时提前返回丢掉坏数字
    披露（persist 规则是 consulted 即披露——Codex 32-#1 同源），且从不调
    `unsupported_corrections`。修=两披露前移/补齐。
  - **R12-03（MEDIUM）登记**：xmlns 冲突门无作用域栈，嵌套局部重绑触发全文
    拒绝。按门自身文档这是有意保守面（拒+披露+无已知生产者触发）；作用域化
    命名空间解析=独立硬化项，登记。**→ 第十三轮实修销账（`5b61687` +
    复审闭合 `3f3df86`，见「当前状态」首条）。**

  6 新测试 + 变异轮 MR1-MR7 恰红 9 条（MR5 覆盖 3 条 denoise 验收，预期
  集合内）；E2E 26/26 重跑全过（heal 预检文案随解码检查更新）。终态
  420 lib + 71 GUI + 7 CLI；累计等集 +52/−0；全门绿。ARCHITECTURE 的
  If-Match 段改述 develop_revision 折叠语义。

- **第十二轮·阶段 6 批 BA：配置信任面重构 + 推理等级 + 动态模型检测
  （2026-08-11，未发版）**

  用户令："先把 provider / base url / key 这些安全类问题解决掉，要优雅彻底；
  设置里能调 OAuth/API、Model，最好再加推理等级。" 三项用户拍板（AskUserQuestion）：
  ①单一能力表全量重构 ②推理等级两个角色各一个 ③放宽过滤 + 分析端独立拉 + 首开自动拉一次。

  - **单一能力表（`config.rs`）**：`Trust{Secret,Destination,Preference}` ×
    `Source{Trusted,DotEnv,WorkingDirFile}`，策略压成一个 `may_supply` match；
    `SETTINGS` 表逐条声明每个设置的信任级与其 `LocalSettings` 绑定字段。
    三份手写名单（`AMBIENT_UNSAFE_VARS` 17 名 / `without_ambient_authority`
    4 字段 / `names_any_ambient_field` 同 4 字段）与 **13 个 `pre(i)` 位置下标**
    全部废除，改为按名查表。旧下标是本轮改动的地雷：删一个名字，8 号之后
    所有字段静默改读隔壁变量。新增两测：表覆盖漂移哨兵（非测试段每个
    `"AUTOSHOP_*"` 字面量必须在表内）+ 重复行拒绝。
  - **L10-2 兑现文档（用户定夺）**：六个模型/供应商名回到 `Preference`，
    `.env` 恢复选模型能力（README:178 / ARCHITECTURE §3 一直如此承诺），
    对合法 `.env` 模型名的无谓警告随之消失（警告集由表派生，不再手写）。
    边界写进代码注释：照片包 `.env` 可切 `oauth→api`，但 base URL 仍是
    `Destination`；而 `.env` 供 key 本就是既定契约，且 `OPENAI_API_KEY`
    路由的是**带照片**的提案器——切供应商不构成能力扩张。
  - **L10-1 信任根**：`store_root` 的 per-user 目录过去在所有平台都取
    `%LOCALAPPDATA%`（Unix 不设该变量），故 Linux/macOS 一律落到
    `/tmp/autoshop` 并把那里的设置文件当**中央可信**（含 key 与 base URL）。
    改为分平台（`XDG_DATA_HOME`→`HOME/.local/share`；相对路径按 spec 忽略），
    并新增 `RootTrust::SharedFallback` 标签 + `SettingsOrigin::SharedRoot`，
    共享 tmp 根一律按环境态降权。
  - **L08-1/L08-2 密钥泄漏（根治+纵深）**：`header_safe_key` 在配置边界 trim
    并拒收不能进 HTTP 头的 key（ureq 2.12.1 `header.rs:147` 会把整条
    `Authorization: Bearer <key>` 回引进 Transport 错误 → 进 rationale /
    设置状态行 / 用户粘贴的日志）；`post_ai_json` 与 `list_models` 两处
    Transport 臂补脱敏，与各自 Status 臂对齐；GUI 保存后若 key 被拒，
    面板直接说明（stderr 在窗口化 GUI 里不可见）。
  - **L08-3 子进程端点继承**：`ANTHROPIC_API_KEY`/`_AUTH_TOKEN`/`_BASE_URL`
    入表为 `Destination`（对该子进程，凭据即路由决策），`dotenv_child_env`
    据表过滤；claude.rs 原有 `env_remove` 保留，负责**继承**的那一半。
  - **L10-5 路径泄漏**：设置加载/损坏告警改打文件名 + 文件夹角色
    （`file_label`），不再把 `%LOCALAPPDATA%\<用户名>\...` 打进 stderr。
  - **推理等级（新能力）**：`Config.{analysis,image}_effort` +
    `LocalSettings` 两字段 + `AUTOSHOP_{ANALYSIS,IMAGE}_EFFORT`。线上拼写
    按族分流并**协商降级**：Responses→`reasoning.effort`（与保活的
    `summary:"auto"` 同对象，各自独立掉落）、Chat→顶层 `reasoning_effort`、
    OAuth→`claude --effort`（`low|medium|high|xhigh|max`，实测自
    `claude --help` 2026-08-11）。空值=不发该参数（对不推理的模型是唯一
    正确请求）；值经 `config::effort` 限形（小写/数字/`-_`/≤32/不得以 `-`
    开头）——它要进 argv。旧 claude 二进制不认 `--effort` 时按
    `cli_rejected_effort` 去旗标重跑一次。三测钉线上形状与协商。
  - **L08-10 动态模型**：`is_chat_model` 由「OpenAI 前缀白名单」改为排除法。
    旧实现让"拉取本端点模型"实际等于"拉取 api.openai.com 的模型"：Codex 桥的
    `claude-*`、LM Studio/vLLM/Qwen/DeepSeek 全被筛成 0，选择器静默退回两个
    硬编码 id。**分析端点获得自己的 Fetch 按钮与自己的目录**（过去仅在两端
    base URL 相同时借用图像端列表）；`Msg::Models` 带 `ModelRole` 标签；
    `SettingsForm` 的四个平行字段收拢为两个 `ModelCatalogue`。
  - **首开自动拉一次**：本会话首次打开设置且该角色已有 key 时自动拉取并缓存
    （无 key 不发请求，之后手动刷新）。
  - **`fable` 别名**：OAuth 模型列表与"切供应商是否为 claude 别名"判定改用
    `CLAUDE_ALIASES`（含 `fable`，出自同一份 `--help`）。旧硬编码三元组会把
    用户填的 `fable` 在切换供应商时静默改回 `opus`。
  - 门：clippy 0；测试 374→377（`comm` 集差 0 删 3 增）；i18n 九门 0；
    字体门重生成后 786/786；web/index.html 与 serve 设置路由同步新字段。

  **BA 复审闭合（同日，Codex 信任面 xhigh 只读复审回执）**：#1 十三个 `pre(i)`
  下标逐一对齐判 CLEAN（回执附完整对照表）、#3 信任根降权判 CLEAN；#4 #5 属其
  口径外未跑到，我自行补证闭合（两个设置写入方均走 `update_local_settings`；三处
  `Authorization` 构造的 key 全经 `header_safe_key` 边界校验）。**#2 判 HIGH 并
  成立**：

  - 现象：`.env` 里的 `LD_PRELOAD=./evil.so` 会被递给三个子进程（claude 验证器、
    python 降噪、python 分割），Linux 上 `ld.so` 在 Python 的 `-E` 生效之前就装载
    它。**非本批引入**——被替换的 17 名单同样不含该名字，重构原样保留了这个洞。
  - **根因（复审点破，比现象更上游）**：`Source::DotEnv.may_supply(trust_of(k))`
    这一个谓词被拿去回答了两个性质相反的问题。它本来回答"`.env` 能否决定
    **Autoshop 自己的这个设置**"——那是**封闭集**，未登记即不存在，故"未知从宽"
    安全；而 `dotenv_child_env` 用它回答"这个**任意名字**能否递给子进程"——那是
    **开放集**，未登记恰恰囊括了平台定义的全部加载器钩子。同一个默认值，前者正确
    后者灾难。
  - 修法（一次系统性改动，非枚举攻击面）：子进程环境块改为**白名单**
    `CHILD_ENV_PASSTHROUGH`，准入规则沿用同一套三分法——只收**选择计算行为**的
    名字（`CUDA_VISIBLE_DEVICES`/线程数/离线开关），凡涉路径、主机、凭据、代码
    加载一律不收。这**故意**排除了旧注释宣传的 `HF_HOME`/`TORCH_HOME`（被重定向
    的缓存＝投毒模型路径，正是 `AUTOSHOP_DENOISE_CACHE` 判 `Destination` 的同一
    理由）与代理变量（代理决定字节去哪）。代价可控且可恢复：全仓无 `env_clear`
    亲证，子进程本就继承父环境，用户自己设的 `HF_HOME`/`HTTPS_PROXY` 照常抵达；
    被拒的只是 `.env` 的追加/覆盖，且**逐名告警**不静默。
  - 测试：`dotenv_parsing_never_mutates_the_process_environment` 的子进程夹具
    扩为五名（正向锚点 `CUDA_VISIBLE_DEVICES=1` 证明 .env 确被解析并放行，
    `LD_PRELOAD`/`HTTP_PROXY`/`HF_HOME`/`PATH` 四条阴性），并断言父侧知识的
    `AUTOSHOP_SIDECAR_TIMEOUT_SECS` 也不再推给子进程。
  - 门：clippy 0；测试 377 全绿（`comm` 对 AS 基线累计 0 删 6 增）；i18n 九门 0；
    字体门 786/786。

  **BA 复审闭合之二（同日，Codex 推理协商／模型过滤／测试质量 xhigh 只读复审；
  前三次死于 harness 10 分钟前台等待天花板，照本轮 L13 旧例改 `--background`
  提交后 11m57s 回执）**：5 发现，回执已按要求给出最上游成因并归并为 A/B/C/D
  四组；亲证后合成三次系统性改动。

  - **ROOT A（1 MEDIUM + 1 LOW 测试，同源）：归因按命名空间，而非按旋钮。**
    根因在 `error_blames_param` 的点号子级规则（`mod.rs:621`）：`/responses` 上
    tier 与 liveness summary 是同一个 `reasoning` 对象的两个孩子，父级判据对
    两者同时为真，而 summary 分支在前——端点明说 `param:"reasoning.effort"`
    时，它丢掉端点从未抱怨的进度流、把端点抱怨的 tier 原样重发，第三个 POST
    才到位。修法：新增 `blamed_child`（父与子分开问），端点**点名**的孩子进
    自己的分支；只有**裸** `reasoning` 才算真歧义，那里仍是 summary 先让——
    它是我们自己的把戏，tier 是用户的明示选择。
  - **ROOT C（LOW）：一次逻辑验证有两个互不通气的状态机。**
    能力认知原是 `post_ai_json` 的局部布尔，随一次调用消亡；而 temperature 钉
    在它**外面**协商，重试从零开始，把刚被拒的 effort/stream 再报一遍——每条
    拒绝多花一个 POST 学第二遍（一次 verify 最坏 6 个，tier 出现前是 4）。
    修法：`Refused{summary,effort,stream}` 改为**调用方**的值，
    `post_ai_json_with` 收 `&mut Refused`，`post_ai_json` 退化为传
    `Refused::default()` 的包装（其余四个调用点零改动），openai_verify 的两次
    尝试共用一份。**"2xx 之后绝不重发"的计费不变量本身复审判 CLEAN**。
  - **ROOT B（MEDIUM）：名字过滤只能剔除它认得的东西，而它按大小写敏感匹配
    原始 id。** `Qwen/Qwen2-Audio-7B-Instruct` 里的 `Audio` 躲过小写 `audio`，
    而注释与测试都宣称排除项"vendor-wide 有效"——实现恰好在**放宽过滤所服务
    的那批第三方 id**（厂商大小写格式）上失效。修法：两个 id 判据统一折叠
    大小写；模态排除之外新增一组**端点族**排除（`babbage-002`/`davinci-002`
    两个只服务 `/completions` 的旧基座，精确 id）；文档与断言口径改回它真能
    决定的事——"是否**上架**"，不是"是否具备视觉能力"。
    **与复审的一处分歧（亲证后不采纳）**：回执把 `Llama-3.3-70B-Instruct` 记为
    误报（纯文本、无视觉）。但分析角色的验证器**从不发图**（`openai_verify.rs`
    模块头「Text-only … never the image」），纯文本 chat 模型在该角色下是正确
    候选；按视觉能力过滤反而误伤另一个角色。保留为正例并在测试里写明理由。
  - **ROOT D（LOW）：测试边界落在协商包装之下。** 旧测试直接驱动
    `verify_command_with` 与 `cli_rejected_effort`，于是"`verify` 是否真把配置
    的 tier 传下去""重试是否存在"两条都看不见。修法：新增 `refusing_stand_in`
    桩（像 clap 那样 exit 2 + `unexpected argument`，**追加**记录每次 argv），
    新测试走 `verify` 真产线、跑两个真子进程。顺带修 `stand_in` 的 sh 分支：
    `printf '%%s '`（Rust `format!` 不处理 `%`，到 sh 是字面 `%%s`）从来只写出
    "%s %s"，argv 值根本没被记录——非 Windows 上读该文件的断言全是空转。
  - 三条新测试逐条**反向变异亲证**（改回缺陷 → 对应断言红、且只红对应那条）：
    第二次 POST 体退化为 `{"reasoning":{"effort":"high"},"stream":true}`；第三次
    POST 体又带回 `reasoning_effort:"high"`；`--effort` 永传 `None` 时只 spawn
    一次。
  - **覆盖缺口（登记，未闭合）**：回执自陈 (a)(b)(e) 未跑到。(a)(b) 已由前一次
    信任面复审判 CLEAN；**(e) GUI 目录失效／`fetching` 标志与重绘泵／首开自动
    拉取能否熬过 `load_settings_form` 重建，至今无任何独立复审**。
  - 门：clippy 0；测试 380 lib + 5 CLI + 55 GUI 全绿（`comm` 对上一提交
    0 删 3 增）；i18n 九门 0；字体门 786/786；密钥扫描空。

  **(e) GUI 面复审闭合（2026-08-12，用户经 AskUserQuestion 选定补审；Codex
  gpt-5.6-sol xhigh 只读，`--background` 提交后回执）**：6 发现（1 HIGH /
  4 MEDIUM / 1 LOW），7 问中 6 问判有缺陷、1 问（settings_ui 借用/顺序）判
  CLEAN。逐条亲证全部成立，按最上游成因归并为**五组根因**（F1 两面同根）。

  - **ROOT I（HIGH，F1 两面）：互斥的提供商共用一个 model/base/key 槽，槽不记
    得内容物属于谁。** 凭证面：OAuth 翻转把 base 自动换成本地桥，而存储的云
    key 仍然上膛——下一次图像调用把 `Bearer <云 key>` 发给桥端口上监听的任何
    进程（反向翻转则把桥 token 送云端）。修法（不变量层）：**存储的凭证绑定它
    保存时的端点**——两个写入端（GUI 表单 + serve POST）打 `*_api_key_base`
    归属戳，`config::file_key_for` 在解析时拒绝把 key 发往别处，`fetch_models`
    的探测同守此规；环境变量 key 不受限（用户自己的明示配对）；旧存档无戳=
    放行，直到一次 base 稳定的保存或重新键入记下归属（`reconcile_key_homes`
    统一在 `update_local_settings` 内治理，base 变更的保存**不得**祝福新配对
    ——残留已在注释与 README 写明）。模型面：CLI 接受别名**和完整 id**
    （config.rs「alias/id」、claude.rs 原样传递），翻转分类却按「非别名⇒非
    Claude」把合法 `claude-opus-4-6` 改写成 `opus`。抽出可测的
    `analysis_model_on_flip`：只改写**确证属于对方词表**的值（API 向仅别名；
    OAuth 向放行别名与 `claude-*`）。
  - **ROOT II（MEDIUM，F2）：显式空白死在 `pick_opt`。** 两个写入面都把清空的
    effort 存成 `""`（=「让供应商决定」），通用 `pick_opt` 先滤空再落环境——
    环境 tier 一设，这个字段唯一存在的那个选择恰好表达不出。修法：effort 走
    专用 `explicit_or_env`（缺席⇒环境可补；显式空⇒连环境一起沉默），语义写进
    LocalSettings/Config 字段文档与 README。
  - **ROOT III（MEDIUM，F5）：web 表单的权威是启动时缓存。** serve 的 `cfg`
    只在自己 POST 后换新，GET 一直端出开机快照；两个客户端保存时又无条件提交
    全部非密字段——桌面 GUI 存的 `high` 被 web 端一次无关保存的
    `image_effort:""` 抹掉。修法：`api_settings_get` 改读文件现值
    （`Config::load`），缓存仍只在 POST 换（喂 AI 调用的语义不因 GET 变）；
    残留=两表单同时开着的固有窗口，GUI 对称路径同理（表单语义使然，已记录）。
  - **ROOT IV（MEDIUM，F3）：拉取的完成没有身份。** 目录只记 `from_base`，
    `Msg::Models` 只带角色——K1 发起的航班在换成 K2 之后落地照装，URL 相同便
    把 K1 的清单顶着 K2 的名义端出；换 key 后重开窗口同样端旧单。修法（与
    serve 目录代同构）：`ModelCatalogue.generation` 由每次 `clear()` 递增、
    `fetch_models` 捕获进消息，落地代不符=丢弃（单飞故 `fetching` 无条件清）；
    另加 `from_key` 凭证指纹（DefaultHasher，仅存内存），`load_settings_form`
    重开时对今日 key 比对、不符即清——web 端换 key 的那半张脸由此闭合。
  - **ROOT V（MEDIUM，F4）：worker 事件不叫醒事件循环，而泵门在拉取起飞之前
    已经跑完。** mpsc send 不唤醒 egui；`poll_workers` 在 update 顶部决定要不要
    续 100ms 泵，同帧稍后 `fetch_models` 才把 `fetching` 置真——指针不动，
    「fetching…」画不出来，完成也躺在信道里等下一次输入。修法（上游一次）：
    `spawn_worker` 自己在两端请求重绘（起飞端排下一帧；完成端由 worker 线程
    `ctx.request_repaint()`），app 持 `egui_ctx`（创建时接真实件，测试用无头
    默认）；100ms 泵保留为兜底。
  - **ROOT VI（LOW，F6）：一个全局 autofetched 在任何角色够格之前就被消费。**
    首次打开设置（哪怕没有任何 key）就花光全会话机会，五分钟后存好 key 的角色
    再也不被自动探测。修法：守卫入 `ModelCatalogue` 按角色分立，**在 dispatch
    时消费**（`fetch_models` 置位，手动拉取同样计数）；复审确认的另一半——
    绝不反复探测计费端点——原样保持。
  - 八条新测试两轮**反向变异亲证**（M1–M8，逐一改回缺陷）：第一轮 lib 侧 4 变异
    → 恰红 4 条 config/serve 新测试、其余 380 绿；第二轮 GUI 侧 4 变异 → 恰红
    4 条 GUI 新测试、其余 55 绿。全部逆向恢复后三套 384+59+5 全绿。
  - 门：clippy 0；`comm` 对上一提交 **0 删 8 增**；i18n 九门 0（新增 1 条丢弃
    提示 + 修订 1 条 fetch 提示的中英对）；字体门 786/786（无新字形）；密钥
    扫描空。README 新增「key 记得自己为哪个端点保存」段并修订 effort 段。

- **第十二轮·阶段 1–3 全清：在册 15 簇归零 + Codex 簇群复审闭合（2026-08-11，
  未发版；计划见下方「第十二轮计划」）**

  阶段 3 尾部五簇（本日十批提交，`971960e..4a59d08` 全推送）：

  - **L14 测试质量 4（`971960e` 批 AC）**：付费 POST"恰一次"从死 grep
    （needle `transport_retried` 早在 R12 b48 被删，空转两轮）改为
    tiny_http 存根端点**计数真传输**（advisor 3 测 + generative 多部分
    重试 1 测，脚本耗尽即关监听）；claude.rs 抽出 verify_command 纯移动
    + argv 隔离契约两测（精确序列/--bare 缺席/API key 剥除/中性 cwd）；
    junction 循环夹具改**强制**（symlink→mklink /J 双跌落，双败 panic，
    别名路径断言）；引导滤波公共入口过产线 1024 瓦片 + 敌意 eps 地板。
  - **L12 i18n 4（`3b6903c`/`f38afe8`/`c24971a`/`f2a888a` 批 AD–AG）**：
    #6 审计第 7 门 zh 占位符多重集比对（自检哨兵+负控实测）；#2A 判词
    类型化 (Decision,reasons) 绘制时翻译（starts_with("Accept") 配色陷阱
    一并拆除）；#2B 确定性 rationale = 24 个模板键入 `src/rationale.rs`
    keys 模块（审计机械抽取），Note 随行 FitReport/HealReport/
    produce_recipe 三返回，**后缀契约**：notes 渲染 = rationale 串尾，
    GUI strip_suffix 命中则本地化、失配整体回退英文（用户拍板"静默英文"；
    持久化五面英文字节不动，金测钉住）；#4 七个 worker 结果全类型化
    （ExportOutcome/RetouchNote/StyleNote/PasteOutcome/(n,dir)/FitNote），
    落地时用落地语言渲染 + 审计第 9 门禁 spawn 闭包内 tr/trf；#3 非 CJK
    文种运行时回退（Windows 实测 micross/Nirmala/malgunsl，mac/linux 表
    留空不臆造），字节帽 stat-先行 24MiB/64MiB，开夹每文种一次性 tofu
    披露（en+zh）。
  - **L04 数值 3（`029ab3f`/`dcbc0d6` 批 AH/AI）**：#1 校准验证器——
    非有限项/退化行/白点加权乘积矩阵行归一 det<1e-4/WB 非正即拒（rawler
    1/0→inf 曾穿过 [0].is_nan 唯一护栏，to_u16 饱和成全黑全白 Ok 交付）；
    #3 风格检索双保险（±1e3 物理带宽入门 + f64 累加 + total_cmp 全序）；
    #2 CA 复合填充缩放 geometry_fill_scale（LUT 节点网格上取
    green×max(ca) 带内最大，≥1、无越界恰为 1 逐位不变），三 LUT/RGBA
    叠加层/正反 norm 映射五消费点同除一标量（用户拍板改像素）。
  - **L09 CLI 3（`fd7fcdd` 批 AJ）**：preflight_out（只读护栏+目录拒+
    父链建）前置**每个付费命令**（analyze/-o、auto 另加格式预拒、
    reimagine/retouch 库函数首句）；heal --full-res 文案照实（"仅 RAW"
    自 b4c6c30 即假）+ GUI 复选框对烘焙源解禁（用户拍板，默认不勾）；
    subset_gui_fonts.py 改「查→建→验→原子发布」+ --check 模式 +
    勘误三处。
  - **L16 setenv 1（`5321f79` 批 AK）**：.env 改 **owned map**
    （dotenv_iter 一次解析，进程环境永不写，unsafe set_var 连根拔——
    其安全注释"主线程首载"前提对 GUI 二进制为假）；env_or_dotenv 供
    五个域外消费点，dotenv_child_env（滤 17 保护名）显式喂三个子进程；
    覆盖优先级/子进程可达性逐项保持；must_cover 从常量自导出（旧手抄
    冻在 14 条时常量已 17 条）；顺带免费关掉 LOCALAPPDATA 未列名旁路。
  - **Codex 簇群复审闭合（`4a59d08` 批 AL）**：9 发现采纳 7——F1（高）
    CA-only 填充动帧但四个几何消费点（serve 掩膜反变形/GUI 锚点映射/
    真轮廓/涂抹叠加缓存门）仍当恒等 → 统一
    `render::geometry_moves_frame` 谓词；F2（高）heal 先付费后验 -o →
    preflight 前置；F3 验证器改判**真被求逆**的白点加权乘积矩阵；F4 浅
    半第四 WB 系数校验；F5 半 .env 解析错误止于首行；F7 MAX_NOTES 溢出
    植毒丸防截断误本地化；F8 子集器 CJK 缺口发布前即致命；F9 夹具目录
    带 pid。登记 3：dotenvy $VAR 插值域偏差（本项目 .env 无插值）、真
    4 色中间态仍后置拒绝、部分粘贴 ● 保守常亮（AF 前同款安全方向）。

  **阶段 4 导出正规化（`0c85dd4` 批 AM，2026-08-11）**：ExportFormat 五态
  枚举（16/8 位 TIFF、16/8 位 PNG、JPEG）替换 save_jpeg 布尔——位深是
  **导出设置**而非扩展名属性，ExportOpts.eight_bit 贯穿 render_to_file
  的 TIFF/PNG 臂（四组合落盘深度测试钉住）；Prefs 持久化格式码 + 旧
  save_jpeg 并写（降级兼容），旧 Prefs 的 save_jpeg=true 迁移为 JPEG
  （往返测试）；Download… 记忆上次导出目录（Prefs.last_export_dir，
  对话框在此重开）；格式下拉五选（label→tr，审计抽取，第六变体无 zh
  对照即门死）；JPEG 质量滑杆恰在 JPEG 时启用；i18n +4 对。
  （对话框/色彩空间/长边/锐化/质量本体系 gap batch F 既有，本阶段补
  位深与记忆缺口。）

  **阶段 5 GUI 美化+手感（`9c2d9db`/`12e018b`/`1c11b64`/`1957efc`/
  `a4ec8a7` 批 AN..AR，2026-08-11，GUI 解禁真机截图自验，改前改后
  四景留档会话 scratchpad before//after/）**：

  **AN 旗舰根因（本轮最大发现）：主题从未真正上屏。** egui 0.29 默认
  `theme_preference:System` 且 `set_style` 只写**当前活动主题槽**
  （memory/mod.rs style_mut 亲证）——浅色 OS 上启动闭包把全套样式写进
  dark 槽、首帧起屏幕渲染 **stock light**：金色系、CJK 13px 字阶、圆角、
  悬停反馈全被静默丢弃，Settings 主题选择器形同虚设；第十一轮 E2E 截图
  "亮色主题"就是此 bug 照面无人识破，**用户点名"高级感/现代感"的第一
  根因即此**。修：install_theme 先钉 theme_preference 再 set_style，并发
  ViewportCommand::SetTheme 同步 OS 标题栏；头less 对抗测试（OS 每帧报
  相反模式，断言 dark_mode + 自家 selection_stroke 存活）+ 阴性对照
  （拔 pin 转红）。改前改后真机对比：stock 蓝选中白面板 → 深色金调。
  AR 补款：创建闭包发的 viewport 命令在事件循环前被丢——首启无 prefs
  不会二次 install，标题栏保持 OS 浅色；update() 首帧补发一次
  （os_theme_synced），真机复验。
  **AN 视觉令牌**：SPACE_XS/SM/MD/LG + RADIUS_SM/MD/LG 统一收编各处
  字面量；dev_hsl 双栅栏（12px vs 兄弟 6px）修复；Tone & WB 补齐栅栏；
  组界"栅栏+细线+喘息"节奏注释为设计；窗口/弹出改垂直软阴影（暗主题
  更高 alpha）；toast 加 popup 阴影+文字色细线描边；中央画布 canvas_bed
  低面板一档（凹陷舞台）；变体条光晕与描边同心（外径=内径+expand）。
  **AO 图标+空态**：第四种关闭符 ×→✕、✅ Done→✓ Done（几何勾家族）；
  空态阶梯统一——控制面板迷你 hero（图标+未打开照片+下一步指引）、
  空文件夹≠未开文件夹两句话、零蒙版一行出处提示、首显影 spinner+
  "正在准备预览…"（裸"…"曾读作卡死）；图库批量行 wrap_mode Extend
  修"渲染选中"逐字竖排（AN 改前截图在案）。
  **AP 快捷键**：+/−/0/1 缩放四键（1:1 经 zoom_one_to_one 每帧孪生，
  按钮与键同源）；Ctrl+Shift+C/V 镜像复制配方/粘贴到选中（LR 的
  Copy/Paste Develop Settings；键位与实现经 AS 修正，见下）；↑/↓ 图库
  走图（controls_rect 悬停让位滑杆微调，本处理器先跑、无条件消费会
  吃掉 nudge）；滑杆右键=双击重置；备忘表 30→33 行；头less 真帧驱动
  测试（步进/12× 顶格/适配回中/1:1 跳转/无照惰性）。
  **AQ 动效**：离散缩放跳变（Fit/1:1/四键/双击）只设 zoom_target，
  update() 经纯函数 util::glide_step 指数滑翔（~45ms 时间常数、dt 钳制
  防卡顿过冲、1e-3 内精确落靶终止）；滚轮保持即时并同步 target；跨照
  重置双写不滑翔；pan 不动画——view_uv 每帧钳制对滑翔 zoom 免费送出
  平滑；双击 1:1 臂顺手修掉仅在 zoom≈1 成立的旧公式（改用不变式）。
  toast 时长随阅读量（40 字后每字 +35ms，封顶 10s/14s，按字符非字节）、
  点击即消（手型指针）、末 300ms 淡出（填充/描边/文字/阴影同步），
  淡出中逐帧重绘、平时 200ms 轮询。
  **真机验证（PrintWindow 后台捕获，全程不碰前台/光标——用户当时在
  全屏游戏，harness 从 SetForegroundWindow+SendInput 重写为 PostMessage
  纯后台式）**：四景改后截图（首启空态英文/图库中文暗/→键开图显影
  面板/图库中文亮），→ 键真机走图开出 01-sunset 即 AP 特性实证；light
  主题奶油金选中+浅标题栏、dark 深色金调+暗标题栏各就各位。

  **AS Codex 复审闭合（`7f91038`）**：Codex 只读复审全阶段 diff 交回
  2 确证 + 1 风险 + 7 项 OK（含主题钉定时序/双槽安全/滑翔终止/toast
  索引/箭头所有权全查证）。F1 确证且比报告更糟——egui-winit 把一切
  Ctrl(+Shift)+C/V 吞成 Event::Copy/Paste **不发 Key 事件**（亲证
  is_copy_command 提前 return），AP 的 consume_key 绑定真机上永远打不
  中，且当时的头less 测试直推 Key 事件测了个假；改为事件层 shift 和弦
  （Ctrl+Shift 抽走事件，裸 Ctrl+C 原样放行给 egui 标签选择复制，
  Codex 预警的撞车根除），新测试喂 winit 真实发的事件形态双向断言。
  F2 确证——zoom_one_to_one 的 vis/disp 式仅在面板受限时不变，
  fit_in 4× 放大帽让小图目标随实时 zoom 漂移；改在 FIT 几何求解
  （w_tex=vis×zoom 真不变式），面板受限时值恒等，fit 已粗于 1:1 的
  图正确钳回适配。F9 风险登记不修——极窄图库面板下超宽按钮裁切，
  240px 默认宽度下 Extend 换掉逐字竖排是正确取舍。

  门（每批全过）：测试 371+5+55；等集累计 **+38/−0**（快照
  current_names_AB→AS）；clippy 0；i18n 审计 9×0（zh 603 条）；字体
  --check 779/779（+准+针）；每提交秘密扫描零命中。**余下：阶段 6
  16 路全扫 → 阶段 7 收口发布 v0.23.2（轮末 GUI 禁令恢复）。**

- **第十一轮：队列四项 + 16 路并行全扫收敛 + GUI 首次真机 E2E（2026-08-09，
  v0.23.1）**

  **队列四项（`bc2be17` + `d8a8b64`）**：①W20 恢复路径披露——
  read_saved_develop 返回 4 元组携带恢复期 ClampSummary，开照/load_version
  上屏 toast；②架构 b persist_postponed——统一失败映射原语接管 5 个位点，
  且 Busy/Io 分型（此前磁盘满也谎称"另一进程占用"）；③架构 c
  ValidatedRecipe——render 4 入口收敛为一次构造+disclose() 的校验令牌，
  ClampSummary 扩到曲线点/字符串字节；④W22 瓦片化引导滤波——
  refine_mask_guided_tiled 1024 边长瓦片 + 6r 晕圈（两级串行模糊×3r，Codex
  纠正了 3r 简报），61MP 全幅 ~3.4GiB 临时平面 → 峰值 ~42MiB，接缝测试全
  像素 ±1 灰阶等价。**Prefs 沙箱缺口（`703ac1f`，我审抓出）**：eframe
  persistence_path 不随 AUTOSHOP_DATA_DIR——E2E 的 GUI 真读了用户图库并
  改写真实 Prefs；现沙箱变量覆盖整个应用。

  **16 路并行 debug 扫描（用户令"十六路并行，每一路完整扫"）**：16 个
  codex exec gpt-5.6-sol xhigh 并发，各持一镜头（并发/内存/耐久/数值/XMP/
  GUI 状态机/安全/披露/CLI/serve/子进程/i18n/跨面/测试质量/本轮差异/Rust
  健全性）互相盲扫全 crate，产出 104 发现。去重+逐条对源码亲证 → 17 项
  本轮修复（F 系列）+ 其余簇全部有据登记「下轮队列」（见上节，带路号）。

  **17 项修复（单提交落地）**：F1 ClampSummary::absorb/describe 四字段全链
  （gui 3 累计点+2 消息、main/pipeline/openai 消费端——curve-only 损失曾
  toast"0 蒙版 0 组件"假全清）；F2 bundled_helper() 边车脚本/权重缓存按
  exe 树解析永不认 cwd（照片包携带 python/denoise.py 即可被执行）；F3
  AMBIENT_UNSAFE_VARS 14→17（PYTHONPATH/PYTHONHOME/AUTOSHOP_DENOISE_CACHE）
  + 两边车 python -E；Responses 三体 store:false（植键包否则可在攻击者
  OpenAI 账户日志读回用户照片——.env 供键契约不动，测试钉住）；F4
  remote_or_device_path() UNC/Verbatim/DeviceNS 词法先拒（exists() 探测
  本身即出站 SMB 认证）于 pixels.json origin/蒙版引用/变体 origin 三处，
  本地盘符绝对路径保留（writer 合法写出）；F5 serve 全 POST 路由要求
  X-Autoshop-Token（fetch 包装器注头+页面占位符替换）；F6 重绘门加
  master_loads（冷变体母版解码完成曾滞留到下次输入）；F7 batch fail>0 /
  eval 全败改非零退出；F8 match -o==xmp_target 先拒（曾两行 success 但
  产物被覆盖）；F9 match 清像素链失败改硬错误（曾静默留旧 master 链）；
  F10 unit_interval 解析器护当时 4 个 0..=1 旗标（--style NaN 曾静默禁用；
  R23-3 起共 6 个——analyze/auto 各加 --strength）；
  F11 中文两处（曲线标题丢近似告警、提供商范围漏填充/重绘）；F12 XMP
  曲线越界坐标改判 unparsable 整组拒绝+披露（"999,-5"曾饱和成近黑单点
  曲线并随保存持久化）；F13 色轮 hue 在场不可读→成对清零 sat（bogus 曾
  落 hue=0=红+sat 50=强红渐变）；F14 generative PNG 头部尺寸前置检查
  （压缩炸弹曾全解码后才比尺寸）；F15 /api/* 全响应 no-store（缓存的
  recipe 404 复用曾可让下次保存清掉新显影）；F16 VariantsRead 三态 +
  refuse_unresolved_strip() 保存拒删门（不可读 variants.json 曾被一次
  普通 Ctrl+S 连 .bak 一起删掉=后台变体团灭）+ 开照 toast；F18 测试补
  MAX_ALLOC 边界谓词（提取 allocation_over_ceiling）与 RGB8 ICC 像素必
  变断言。F17（openai schema maxItems）**据 openai.rs:308-311 已验证
  API 限制（strict 模式 maxItems 400）驳回**，披露地板并入 F1。

  **E2E 18 项（真 release 进程+沙箱 store）**：超限披露/极端几何/降噪门/
  缺栅格点名/ICC 收+拒/serve 令牌三态/**POST 无令牌 403+带令牌 200**/
  **no-store 头**/GUI 真机启动 10s 无恐慌+全屏截图+干净退出+**Prefs 落
  沙箱断言**。GUI 首次真机运行（用户本轮解禁）：中文界面零豆腐块（嵌入
  字体首次实战验证）、亮色主题、图库 122 缩略图正常，截图已交用户作
  Phase 1 视觉验收材料。

  **Codex 复审（gpt-5.6-sol xhigh 只读全 diff；子代理会话无该模型即如实
  中止，改 16 路同款直连管道重发）**：2 发现均亲证属实并闭合——#1
  /api/recipe 的直构 404 绕过 no_store()（缓存的"未分析"回放会重开中性
  画布）；#2 bundled_helper 在 current_exe 不可用时回退 cwd 相对=洞重开
  → 改 NUL 字节哨兵 fail-closed（任何平台真实路径都无法包含 NUL，攻击包
  永远无法满足）。E2E 硬杀缺陷同修：terminate()=TerminateProcess，eframe
  只在优雅关窗写 prefs → 改 CloseMainWindow 后隔离断言真过。

  **终态**：281 lib + 3 CLI + 37 GUI 全绿（第十一轮起点 273+2+37），
  clippy 0（--all-targets --features gui），audit_i18n.py 0，字体门 669
  CJK 码点，E2E 18/18 全过（含 GUI 优雅关窗+沙箱 prefs 落盘实证）。

- **第十轮：GUI 打磨五项 + 双轨攻击式审阅收敛（2026-08-09，v0.23.0）**

  **Phase 1（`86d3dcc` + `c3b7f92`）**：五项全落——嵌入五个 Noto 子集字体
  （符号+CJK 663 码点，`embedded_fonts_cover_every_ui_symbol` 门禁钉死，
  scripts/subset_gui_fonts.py 从源码字面量自动提取）；亮暗双主题（Prefs 持久
  化）；排版/按钮打磨；i18n 对齐。⚠ **视觉验收只能用户做**（GUI 禁启动纪律）。

  **Phase 2 审计**：Codex gpt-5.6-sol xhigh 三批全 crate（C1-C6 + MARGINAL 16
  + 补扫 C7-C11）= 92 发现 44 确认/48 降级/0 误驳；Claude 结构审 B1-B9 并行；
  合并 36 工作项 W1-W36，13 项设计取舍全部 AskUserQuestion 拍板。

  **修复六提交**：`a9769af`（W6-8 XMP 原语/W22 面/W30-32 含 qcms ICC/W35 边车
  限时限量/W36 修瑕覆盖栅格）→ `c3b7f92`（W18 CJK 嵌入/W28 转义盲区）→
  `b7d4eb2`（W14 serve 身份/W15 导出名注册/W26/W5 .env 信任边界/W21/W23）→
  `f122914`（W11 耐掉电发布/W12 stem 误伤/W13/W25/W24 store 侧 OS 显影锁
  Wait/NoWait+线程本地重入）→ `b75e18b`（W24 GUI 九位点手写：Ctrl+S 复合/
  Save-all 逐照/save_version/Analyze/粘贴 Wait/反推 Wait/delete_version；
  W17/W27 trf 单趟/W29 审计门=动态键源码机械提取+tr() 旁路检测，当场抓 4 个
  存量漏翻）→ `bba3e9c`（W2 refine 携带引擎态/W9 渲染入口净化+range 整组退役/
  W10 栅格快照+总量预算关 TOCTOU/W20 ClampSummary/W33 stage_and_publish/W34
  clap requires；GUI W1 笔刷重排/W3 警告弹回/W4 worker 解码/W16 合成色对比度
  检验——当场抓出暗色选中文字 2.57:1 真缺陷并重调两主题/W19 异步改名提交）。
  **W22 余项（瓦片化引导滤波）用户拍板推迟**（见上方下轮队列）。

  **Debug 轮（用户五步收官令第 4 步）**：Codex gpt-5.6-sol xhigh 只读全差异
  复审（558k tok）11 发现，逐条亲证后全闭合五提交：`e481667`（#1 auto/match
  持一把 Wait 锁罩住 backup→recipe→清链→XMP、渲染锁外；#2 W4 引入的回归=
  stash (base,origin) 双 Some 门丢 origin→Ctrl+S 清 pixels.json 存谎，改
  pending-master 路径保身份保 kind 并重掖解码；#6 解码在途集合+load_image
  限额）、`cbcc0de`（#4 16 位 ICC：qcms 仅 8 位（源码实证）→ 33³ 格 LUT+三线
  性插值 f32 保位深，LR "Edit in…" ProPhoto 16-bit 真正可开）、`601dbd3`
  （#3 模型改名蒙版可双涂→未匹配即保守整体回退+rationale 披露+提示词钉名为
  身份）、`408f6e6`（#5 未决开照→保存先过备份门；#7 栅格解码前尺寸预检×4；
  #10 渲染入口披露丢弃；#11 删 unreadable_mask_rasters 死门、测试迁快照）、
  `e318939`（#8 has_develop 认 .bak/saved_recipe_snapshot 先恢复/自定义 -o 改
  stage+rename 免孤儿 bak；#9 drain 线程 2s 宽限有界 join+披露截断）。架构
  议题 a 删除、b/c 双方同意进下轮队列。

  **终态**：273 lib + 2 CLI + 37 GUI 全绿（第十轮起点 233+1+32），clippy 0
  （--all-targets --features gui），audit_i18n.py 强制门 0。

- **第九轮：用户反馈三连修 + 蒙版大改（2026-08-08~09，v0.22.0 已发布：tag
  `v0.22.0` → 提交 `84be2cf`，release assets 字节验证 autoshop-gui.exe 35840472 /
  autoshop.exe 27572908，Latest；用户勾选全量修复：问题3退出活锁 + 问题1变体身份 +
  蒙版打磨全项含追加的增加/排除/交叉；发布前用户解禁 E2E 运行时测试）**

  **① 变体身份 + 退出活锁（用户问题 ①③，同一结构性根源）**
  - 诊断：三路调查工作流（11 代理），8 条根因主张全部独立对抗验证 CONFIRMED，
    Codex 只读复核确认并修正两处表述（"永脏"限定于保留变体的正常保存操作；
    Save-all 逃生口必须语义化为保存而非丢弃授权）。
  - 根源：变体带纯会话态。`StashEntry.generated: bool` 压扁三值 kind（导航回来
    「◭ 反推」→「▣ 原片」）；pixels.json 的 `kind: generated|inplace` 2 值标志
    无法表示 Fitted（冷重开同病）；`open_dirty_variants` 拿非活动变体的 origin 与
    **整照片唯一** master 记录、recipe 与**活动画布的** saved_recipe 比对 →
    generate→fit 后背景 Generated 变体结构性永脏；quit 对话框 Save-all 只写
    pending（手动 Ctrl+S 后为空），发 Close 而无 discard_requested 式逃生口 →
    守卫下一帧再拦 → Close/CancelClose 死循环 =「保存并退出」死键。
  - 修法：`store::variants.json`（`VariantsRecord`：others[kind/recipe/origin] +
    active_kind/active_pos；写侧 relativize、读侧 resolve；publish_json_sidecar
    从 write_pixel_source 抽出共享；recover_orphan_baks + clear_develop 注册
    variants 对）。GUI：`saved_strip` 镜像；`open_dirty_variants` 重写为
    live-vs-mirror 位置逐一比对（kind/recipe/origin/删除/活动卡漂移全计）；
    StashEntry 补 kind；冷重开从 variants.json 重建整条带（active_kind=fitted
    直接恢复；generated 仍走 baked pixels 臂；背景变体 base 在 load_active 首次
    切换时惰性解码，失败 toast 降级）；Ctrl+S `persist_strip`（失败=镜像不推进=
    保护仍在）；Save-all pending 四元组带 strip、noop 清除臂加 strip 平凡门、
    成功臂 `inactive_dirty_variants` 残留复查（>0 点名 toast，绝不无声弹回）。
  - 回归钉死：`fitted_kind_survives_the_navigation_stash`、
    `a_persisted_strip_makes_background_variants_saved_work`（前置=修前的死锁态，
    断言=persist 后守卫必放行）；store 侧 `variants_record_round_trips_…`。
  - 语言切换仍会即时改卡片文字（派生标签的固有行为，非 bug）；跨面（serve/CLI）
    对同库照片写 pixels.json 的旧影响不变（variants.json 为 GUI 专属）。

  **② 蒙版八件套（用户问题 ②全量勾选）**
  - **组合**：`LocalAdjustment.components: Vec<MaskComponent{geometry, mode}>`，
    Add=屏并 `1-(1-w)(1-c)` / Subtract=`w(1-c)` / Intersect=`wc` 按序折叠
    （`combined_mask_weight`）；不可读组件栅格=整调整惰性（丢 Subtract 会扩大
    效果区）；三处对称（apply_masks/mask_coverage/unreadable_mask_rasters）；
    consumers 经 `bitmap_paths_mut()` 单一遍历（store 五处 raster 路径处理共用，
    新载体永不漏）。ENGINE-ONLY：crs MaskBlendMode 语义无验证参考边车（roundness
    同律），XMP 只投基形状，面板灰字披露。GUI：Shapes 区（模式选择+▭/◯ 布置、
    行内改模式/删除/选中编辑手柄 sel_component）。
  - **径向旋转**：`Radial.angle`（serde default 0，±180 wrap 消毒）；引擎逆旋
    采样点；手柄=旋转轴端点 + 旋转手柄 id5（顶轴外侧）；拖拽沿旋转轴投影
    (du,dv)，angle=0 时逐位退化为旧行为；描边参数化采样含旋转；XMP 写出
    未旋转椭圆（放置 bbox 同款超集近似，文档+注释双披露；crs:Angle 不映射）。
  - **眼睛开关**：`enabled`（默认 true；引擎跳过、覆盖归零、导出门跳过、XMP
    写出跳过禁用项——CorrectionActive="false" 无验证不猜）；列表行 👁 + 禁用行
    弱化显示。
  - **复制蒙版**：⧉ 按钮，克隆经 `detach_rasters` 拿独立栅格副本，64 上限有提示。
  - **活跃 ●**：`render::engine_active` 提为 pub，列表行与导出门同一条规则。
  - **笔刷蒙版**（新建+编辑）：`mask_brush` 会话骑现有画笔基建（paint_mode 全
    联动）；灰度权重缓冲 `mask_brush_gray` 为真值（RGBA 画布仅显示，防 8-bit
    往返漂移）；擦除=双缓冲清写；Apply 烘焙进 claim_raster 新栅格（输入文件
    永不变——版本快照引用安全）；「编辑栅格」把现有位图播种进会话。
  - **栅格烘焙**：羽化（高斯）/扩展/收缩（可分离 max/min 形态学）每档新栅格；
    **全分辨率精修**=worker 解码全尺寸原图 + He 引导滤波（box 均值走 NR 的
    blur_plane），landing 以索引+存储引用双验证（Codex R9-1），refine eps 引擎
    侧正下限（R9-2）。测试：组合代数逐点、顺序敏感、形态学 5×5/3×3/恒等、
    引导滤波边界贴合。
  - **刻度 0-100**：`slider_pct`（Amount/边缘羽化/亮度范围三杆/容差显示 ×100，
    存储不变=XMP 契约；≥20 宽规则自动整数吸附）。
  - **导入丢弃计数**：`xmp::unsupported_corrections` + `read_saved_develop` 第三
    返回值（独立通道，不混入 xmp_bad 的"数字不可读"语义），打开时 toast 点名
    N 个 LR 笔刷/AI/景深蒙版未导入、边车原样保留。
  - i18n：本轮 40+ 新字符串全部补中文；serve 的 XMP 损失披露措辞已覆盖
    engine-only 编辑（核对无需改）。

  **验证与披露**
  - clippy --all-features --all-targets 0 警告；测试 233(lib)+30(gui)+1 全绿。
  - Codex 只读评审（全 diff）：(a)-(f)(h) 维度明确 cleared；2 发现已修复并复验。
  - **E2E（2026-08-09，用户解禁运行时测试；18/18 PASS，全部 release exe 真进程 +
    `AUTOSHOP_DATA_DIR` 沙箱 + 中性 cwd，脚本=会话 scratchpad `e2e_round9.py`）**：
    - CLI `apply`（像素级断言，512×384 均匀灰 128 合成源）：enabled=false 全帧
      +2EV 蒙版 → 输出与基线**逐字节相同**；排除组件雕刻 → 被雕面 128==基线、
      外侧 205；radial angle 0°/90° → 探针精确对换（0° 时 H=239/V=128，90° 时
      H=128/V=239）。
    - `serve` 真进程（/api/list → gen → POST /api/xmp）：200；沙箱店内产出
      `<stem>.xmp` + recipe.json；XMP 三纪律验实——禁用蒙版整条跳过、组件不投影
      （CircularGradient 计数=1 仅基形状）、无 Angle 属性；recipe.json 无损
      round-trip（angle=37.0、subtract 组件、enabled=false 逐字段还原）。
    - 覆盖边界如实：variants.json 为 GUI 专属消费（真实文件系统级由
      `variants_record_round_trips_…` 等单测覆盖）；蒙版笔刷/精修的 worker 流程
      为 GUI 交互路径，逻辑层由无头 GUI 测试 + 引擎测试覆盖——GUI **窗口**仍未
      启动（"绝不弹窗"标令按 v0.20 先例解释为 CLI/serve 级运行时解禁）。
  - 披露：组件/旋转的 XMP 投影限制见上（解锁条件=一份带 MaskBlendMode/Angle 的
    真实 LR 参考边车）；变体带 persist 只在保存时发生（fit/reimagine 推卡后带未存，
    守卫如实提示直至 Ctrl+S——语义即"未保存"，非缺陷）。

- **第八轮：色调模型正修 + 剩余项清零（2026-08-07，v0.21.0；用户令"开始下一轮，
  把所有剩余项清空"，此前已确认 kelvin 验收、色调下轮攻、计费/GPU 已授权）**

  **① 色调模型：两个已知缺口正式关闭（六轮悬账）**
  - 根因（读码定位到数值级）：`ys[i] = E(x_i, ev) + basis(x_i)·s`——basis 在
    **原始 x 轴**取值，不知道曝光已把顶部基线压平。+1.5EV 时 x≥0.63 基线全夹到
    1.0，contrast −100 在 x=0.66 打出 1.0−0.2·0.705=**0.859**（正是实测平台
    code 56304），x=0.82 偏移更深触发单调回写 → [0.66,0.82] 整段压平 197 输入。
  - 修法=模型层 `tone_knot_weights(ev)`：每个 knot 的滑杆权重 =
    `ramp(0, 0.01, max(左右基线间隙))`；两侧都被曝光剪裁的 knot 失权（曝光的
    剪裁就是剪裁，ACR 同语义），健康侧存活的边界 knot 保全权，ev=0 时全部
    恒等于 1（**逐位不变**，smoothstep 在 t≥1 精确=1）。
  - 连坐缺口同修：`limit_tone_sliders` 改为**按滑杆迭代**——每轮找最坏越界区间、
    只收缩对该区间**负贡献**的滑杆（正贡献/无关滑杆保全权），≤8 轮 + 单 λ
    **无条件兜底**（硬保证不押注收敛）。shadows+50/whites−100：shadows 22.5→
    **50 完整保留**，whites 停在自身容量 −0.45；blacks 反向开区间者全然不动。
  - 实测（同一 270 格栅格、只计内部平台）：13 格>96/最坏 197 → 仅权重 7/103 →
    权重+按滑杆 λ **6/100**（3 格为 65534 量化边缘）。栅格测试撤 220 豁免、
    全域 128 单上界。真片对照（contrast−100@+1.5EV 高光裁片：死灰平带→云层
    渐变恢复，均值差 0.78/255）已邮件送用户验收。
  - fit.rs：权重进 ev 扫描逐候选加权（三处一定义，模型仍线性可逆）。饱和区
    天然简并（ridge 先验偏爱小滑杆等效解），测试钉「解在引擎自身罚分目标下
    不劣于真值」（M-F1 中性化变异灭）；**fit 侧权重本身无可测行为差**（外层
    按真渲染打分的验收环掩蔽之——全套件在该变异下全绿，已实测并在测试注释
    如实注记，权重保留理由=一致性而非测试钉死）。
  - 变异：M-T1（权重全 1 → 栅格 146 翻红）、M-T2（迭代禁用 → 连坐测试翻红）、
    M-F1 全灭；刀锋教训一条：先写的"参数还原"断言在正品上就假阳性失败
    （简并所致），改钉正确性质后变异照灭。

  **② E2E 清零段（用户已授权计费/GPU）**
  - CLI：`heal --no-auto --mask` 本地臂✓（1 spot，2048 工作副本，掩膜映射对）；
    `eval` ✓（1 对，AI-vs-用户差距报告 1.3%）；`batch` ✓（鲜 store 1 pending→
    分析→Accept→存盘；首跑"0 pending"是我从 repo 根跑、仓库自身 legacy ./out
    被按 stem 认作已有 develop——**测试摆位错误，非产品缺陷**，legacy 兼容
    按设计）。
  - serve：`/api/style-build` ✓（1 exemplar）、`/api/download` ✓（24.9MB 全
    分辨率 + RFC5987 文件名）、`/api/analyze` ✓（计费真跑）、`/api/heal` ✓
    （auto 检测计费真跑，1400×933）。
  - `/api/retouch` 挖出**真缺陷并修**：API 以 SSE error 事件流内拒绝柔性尺寸
    `2048x1360`（Supported: 1024/1536 枚举+auto），而尺寸/保真度协商只存在于
    阻塞 400 臂——流式直接终态失败。修=`streamed_refusal_blames`（识别
    `"image stream error: "` 前缀 + `error_blames_param` 结构化归因；**前缀
    必需**：读/解析失败可能跟在已计费生成后面，绝不协商重发——M-G1 变异初版
    幸存暴露反例面不足，补"无前缀但引号提参"负例后灭）。验证拒绝=生成未
    启动，重发无重复计费。
  - retouch 客户端**无罪证明**：严格 mock（逐字节校验 Content-Length=1,656,004
    吻合、email 解析器九 part 全过、v0.19.0 `choose_boundary` 构造正确）下
    全管线端到端完成 exit 0。真端点当日 5 试：4×11 秒传输墙（引擎自诊断
    正确、3 秒计费安全窗拒绝盲重发正确）+1 次穿透即触发上述尺寸拒绝。
    **流内协商的真机全环路复验待网络恢复**——修复本身由真实载荷单测钉死。
  - GUI：仍按用户"绝不弹窗"标令排除运行时验证（如实披露，唯用户可解禁）。

- **第七轮：首次运行时端到端验证（2026-08-07，v0.20.0；用户令"目标完美 bug free /
  允许运行时、端到端测试 / 解决所有问题+全文档+发版"）** —— 六轮以来的"绝不启动
  GUI"约束仍守（egui 窗口即弹窗，且无法脚本化交互，自动验证价值为零）；解禁的是
  **无窗口运行时**：CLI 与 serve 真进程、真 61 MP ARW、真 HTTP。

  **① E2E 四层，全部沙箱化（`AUTOSHOP_DATA_DIR` 重定向 store、独立 cwd，用户真实
  develop 与中央设置零触碰）**
  - **CLI**：`decode`（Sony A7R4A 元数据/直方图/1536px 预览全对）→ `apply`（用户
    真配方双 mask，61 MP 6.3 s）→ `match` 反推（look error 0.112→0.027，统计拟合
    按文档不还原逐滑杆；store XMP 单 Description 带 crs，XML 可解析）→ 反推配方
    （version 2）回 `apply` 再渲染成功 → `recipe-schema` JSON 有效。
  - **serve**：真进程 20/20 断言——静态页、`/api/list`+`gen` 代数戳（无戳/错戳
    mutation 一律 409，这是**正确行为**，测试客户端初版没带戳全军 409 反向证明了
    防换目录竞态门在工作）、thumb≤256/preview>256 真 JPEG、新 store 首开 404 带
    fresh-base 画布、`/api/develop` 真渲染 + `exposure_ev:1e30` 被 clamp 不 500 +
    未知字段 422、跨域 Origin 403、`/api/settings` GET 无 key 泄漏、**外源边车
    合并旗舰实战**（exiftool 风格 Rating/Label 边车 + 保存 → 星级标签逐字节幸存、
    `x:xmptk="exiftool"` 文档身份保留、58 个 crs 属性拼入、响应无假损失提示、
    **库内边车未被写**）、保存后 `/api/recipe` 往返一致、`/api/export` 全分辨率、
    未知路由 404、`setdir` 非目录 400。
  - **安全四件套（真进程 + mock HTTP 监听计数，每个负例配灵敏度正例对照）**：
    ①`.env` 植入 `AUTOSHOP_PYTHON`+`AUTOSHOP_DENOISE_SCRIPT` → canary 未执行（而
    警告行证明 .env 确被读取，是"拒用"不是"没读"）；正例=同变量走可信父环境 →
    canary 执行。②`.env` 植入双 base URL → mock 0 命中，请求走真 api.openai.com
    经假 key 得 401（401 后正确回退启发式基线、confidence 0.4、verdict Revise →
    拒绝自动保存，整条降级链路披露完整）；正例=父环境设 base URL → mock 记录到
    `POST /v1/responses`。③WorkingDir `autoshop.local.json` 植入端点+key → 警告
    打印、0 命中。④被投毒 cwd 里起 serve 存设置 → 中央文件无 `9666`/`sk-planted`、
    良性字段正常落盘（v0.19.0 洗白修复的实战复验）。
  - **E2E 花费披露**：②的两次 analyze 意外触发了 Claude (OAuth) verifier 真调用
    ×2（verifier 与 proposer 是两条配置线，此前未意识到 verifier 会独立走通）；
    后续所有 analyze E2E 已用不存在的 `AUTOSHOP_CLAUDE_BIN` 掐断。

  **② 本轮唯一产品缺陷：denoise 假成功（E2E canary 直接抓获）**
  - 症状：边车进程 exit 0 但不写产物 → CLI 打印 `denoised -> 路径` 并 exit 0，
    该路径上**根本没有文件**。用户会以为照片已去噪保存。
  - 根因：`denoise::run_sidecar` 只验**进程退出码**，从不验**产物**；`segment.rs`
    的 `segment_file` 早有 "`sidecar reported success but wrote no mask`" 检查——
    代码库早知道这条规则，denoise 漏配。`denoise_buffer` 有回读兜底（错误形状是
    困惑的 "open denoise output 失败"），`denoise_file` 线（CLI 去噪烘焙图、GUI
    画布去噪 ≤2048 臂）完全裸奔。
  - 修复（**初版被复审两次击破后的最终版**，见下 ⑤）：契约收归一处
    `lib.rs::sidecar_wrote(who, output, before)`，三重拒绝——文件缺失、文件为空
    （GUI 的 `unique_out`/CLI zoned 的 `store::claim_raster` 都先创建 0 字节
    认领文件，裸 `exists()` 检查对它们**从不生效**）、文件存在但**先于本次运行**
    （spawn 前快照 `(len, mtime)`，CLI 交付名确定性（`out/<stem>.denoised.tif`）
    使上次导出可以就地冒充本次结果）。denoise 与 segment 两座桥同修；
    `denoise_buffer` 另加 spawn 前清孤儿 temp（PID 复用 + 计数器归零可让崩溃
    残留物顶到同名，同尺寸时会被当本片结果解码）。`python/denoise.py` 无条件写
    输出（tmp + `os.replace` 原子落盘，连 strength=0 也先混合再写），故无合法
    "不写"路径，零误伤。
  - 变异实证 ×5，全灭：M-D1（删 denoise 调用）、M-D2（放行空文件）、M-D3（丢
    spawn 前快照 → 陈旧交付物放行）、M-D4（无条件拒绝——复审者指出的"杀死全部
    真去噪但测试照过"变体，由新增的**写入正例对照**捕获）、M-S1（segment 回退
    裸 `exists()`）。真机复验三景：no-write → exit 1 响亮报错；陈旧交付物在默认
    `./out` 名 → exit 1 + `predates this run`（初版此处 exit 0）且陈旧文件保全
    不删；写入正例照常成功。
  - 教训入账：变异实证后用 `git checkout` 恢复会把**未提交的修复本身**一并回滚
    （本轮真踩），改用字节精确备份/回写；`os.replace` 保留旧 mtime 会让 cargo
    增量编译跑**旧的变异二进制**（测试莫名连红两轮），恢复后必须 `touch`；
    Windows 下 `subprocess(text=True)` 按 GBK 解码 cargo 输出会炸线程——变异
    驱动脚本自己崩掉时**变异会滞留在源码里**，恢复必须放 `finally`。

  **③ kelvin 重定标：补上 v0.19.0 欠的视觉+数值双验收（真片，非复制品）**
  - 方法：worktree checkout `104a95d`（v0.18.0 引擎）另行编译，同一 ARW 同一配方
    双引擎渲染。6600K 接缝对（6590K vs 6610K，950px 均值/8-bit）：旧 R−0.673/
    G−0.002/B−1.108 **真跳变**，新 R+0.043/G+0.000/B−0.102 **连续**。幅度：7000K
    新−旧 R+1.473/B+0.968；2000K B−0.980。四张验收拼图（接缝 ×24 差异图、7000K、
    2000K、色调残留）已交用户——**美学最终裁决仍在用户**，但"分支相接"从公式推导
    升级为真片实测。
  - 色调高曝光残留：contrast −100 @ +1.5EV 真片渲染，高光裁片**肉眼可见**云纹
    压平（对照中性显影同裁片纹理清晰）。仍刻意未修（v0.19.0 台账：四候选实测
    全劣、真修法在色调模型层）；本轮把"需要视觉验收"这一欠账变成了**已交付的
    验收材料**。

  **④ 六轮文档漏改（本轮文档核对抓获）**：`91320ab` 给 ROADMAP 加了 164 行六轮
  台账，但横幅与台账标题的"未发版"写于发版前，v0.19.0 发布后从未回改——上一轮
  "全部文档已对齐"的宣称在此处不实。本轮修正（横幅与下条标题）。

  **⑤ Opus 对抗复审（Codex 额度至 2026-08-09 未恢复，按用户既往授权以 Opus 替代；
  独立 Codex 复审本轮未运行）——11 条发现，模式第四次重演：本轮修复自身有洞**
  - CONFIRMED×7 全修：①陈旧交付物冒充本次结果（复审者用真 CLI 实弹演示：初版
    检查对预先放置的 33 字节文件 exit 0）→ spawn 前快照；②两条测试存在"杀死全部
    真去噪但测试照过"的幸存变体（无正例对照）→ 新增写入正例 + M-D4；③"segment
    早有此检查"的辩护注释**不实**——其 `exists()` 对两处调用点预先创建的 0 字节
    认领文件从不生效，segment 是同缺陷的**未修同胞**而非先例 → 同修 + M-S1；
    ④GUI 走认领文件路径只会命中"空文件"臂而该臂无集成测试 → 补；⑤测试夹具
    固定目录名（并发互删+残留翻转断言）→ pid 唯一化；⑥M-D1 在非 Windows 零覆盖
    → sh 替身补齐；⑦`denoise_buffer` 孤儿 temp 冒充（PID 复用）→ spawn 前清除。
  - 复审自身也有一处**过计**：宣称 `/api/recipe`、`/api/fresh-base` 未被 E2E 命中，
    实际二者各有断言（fresh-404 带画布 + 保存后往返）。已对照测试输出核实。
  - 盲区响应：补 `/api/style-info`、`/api/import`（含坏路径 400）、`/api/upload`
    （含穿越名不逃逸）三路由 E2E 与 CLI `style-index`（沙箱 store 建成 1 exemplar）
    → serve 断言 20→25。

  **⑥ 发布后同日：用户授权小额计费 + GPU 的真端到端（v0.20.0 发布说明的
  "not verified" 清单据此收窄，见 release 附记）**
  - 真 SCUNet GPU 去噪：模型加载/分块/写盘全链 7.8 s，输出确为去噪非拷贝
    （对输入均值差 1.85/255）——本轮的三重拒绝契约也随之过了**真边车**的正例。
  - 真 `analyze`：GPT-5.6-sol 提案 + Claude 校验 **Accept 0.9**，52 s。
  - 真 `auto`：提案→校验→9504×5702 全分辨率渲染（AI 提案含裁剪）→XMP 落
    沙箱 store，44.6 s。
  - 真 `reimagine --quality low`：gpt-image 3520×2352，1 974 tokens，40.7 s。
  - 四路径首跑零缺陷。**仍未运行时验证**（如实披露）：CLI `batch`/`eval`/
    `retouch`/`heal`；serve 的 `/api/analyze`、`/api/retouch`、`/api/heal`、
    `/api/download`、`/api/style-build`；GUI 全部（政策）。
  - 用户四项决定（2026-08-07）：①kelvin 四张拼图改邮件送达（SendUserFile
    看不了），验收结论待回邮；②**下一轮攻色调模型**（残留修复，配真片视觉
    迭代）；③计费 E2E 即本条，已执行；④Codex 不补（Opus 覆盖已认可）。

- **第六轮：清账 + 再攻自身（2026-08-06，用户令"开工"；已作为 v0.19.0 发布，
  `91320ab`——本条标题原写"未发版"系发版前措辞，第七轮修正）**
  —— 范围=第五轮记下的五条已知缺陷，外加两个独立攻击子代理（一个攻 `104a95d` 自身，
  一个攻本轮尚未提交的 diff）。共修 **15 条**，每条都有具名变异实证或独立复算。

  **① 清掉的五条旧账**
  - **merge 提示两缺陷**（第五轮记为"让上一轮的披露机制形同虚设"）：`store::read_sidecar`
    的单 `Option` 形状把"文件不存在"和"读不了"折叠成同一个答案，于是超 16 MiB 的
    Lightroom 边车被 `.or_else` **静默换成我们自己上次的投影**当底稿，merge"成功"，
    **真丢失一声不吭**。→ 新增 `store::SidecarRead{Missing,Ok,Unreadable}`；底稿现在
    **自带来源路径**（旧提示一律写 `out.display()`，指向根本没动过的文件）；空白边车
    不再报警；非 Camera-Raw 源不再读邻居 `.xmp`（与 `store::lightroom_sidecar` 的既有
    契约对齐）。**攻击代理指出我只修了一半**：纯评分/关键词边车（exiftool/Bridge/
    Capture One，没有任何 `crs`）走的是 `find_crs_description` → `None` 这条路，仍会
    **每次保存都报一次、永远无解**。→ 根因修法=`xmp::insert_crs_description`：往现有
    `rdf:RDF` 里**插入**我们自己的 Description，用户的星级/标签逐字节保留、我们的设置
    也落地，于是那是一次**真合并**，没有损失可报（测试并钉住"再存一次不会叠出第二个
    设置块"）。
  - **流式路径无总时长上限**：实况比记录的更糟——keep-alive 注释是**字节**，socket 的
    stall 超时每收到字节就重新计时，所以只发注释的服务器/代理能把读取永久挂住，而
    **连 ✕ 取消都拦不住**（取消检查在事件回调里，没有事件就永不执行）。→ `ProgressGate`
    把活性度量从"字节"改成"内容"：只有 `:` 注释行与空行不重新计时（`event:`/`id:` 按
    SSE 规范算真活动；一条几 MB 的 base64 `data:` 行按**字节**计时，不会在传输途中被
    杀）。`start_analyze` 补 `arm_cancel` + `Msg::Analyzed` 带 epoch（迟到结果不再安装、
    更不再**落盘**）。
  - **decode.rs 两条错引用** → 改为函数名引用（行号引用已两度腐烂；实查其中两条指到了
    `thumb_base` 这个毫不相干的函数）。三条断言逐条重验为真后保留。
  - **`part_text` 多部分注入** → `choose_boundary` 按 RFC 2046 §5.1.1 确定性避让。
  - **`kelvin_to_rgb` 6600K 不连续** → 三通道接缝全部 C0 连续（绿 1.31%、蓝 0.96% 悬崖
    消除，红 88K 平台消除；蓝通道双点定标同时钉住 1900K 下端）。子代理独立复算了
    doc 注释里的**每一个数字**，全部吻合。

  **② 攻击代理找到的、必须修的**
  - **HIGH（本轮自己引入）`choose_boundary` 是 Θ(prompt²)**：防注入修复本身成了 DoS。
    实测 1 MiB prompt + 4000 个 `-` = **7.2 s**，而 `RetouchReq::prompt`（serve.rs:1197）
    **无长度上限**、body 上限 256 MiB（serve.rs:2422），跑在请求线程里占 8 个许可之一。
    → 改为**单趟扫描**：找出 base boundary 之后最长的 `-` 连续段，一步跳到 run+1；
    超过 RFC 2046 的 70 字符上限则**拒发**（那样的 boundary 本就非法，静默发出一个
    内容可伪造的定界符正是本函数存在的意义所在）。
  - **HIGH `load_local_settings()` 未过滤**：ambient 守卫只装在 `Config::load` 读路径，
    而这个函数的文档明写"这是保存时要合并进去的基底"，两个写入者（serve 路由 + GUI
    面板）`if inc.x.is_some()` 式合并后写**中央文件**——于是 cwd 文件里的端点被洗进
    可信文件，`Config::load` 再看到就是 `Central`、不再剥离。**一次普通的"保存设置"
    就让整道守卫永久失效**，且本机当前正满足触发前提。
  - **HIGH `.env` 是同一条路且更强**：dotenvy 从 cwd **向上搜到根**，`dotenv_override`
    连用户真正设置的环境变量都盖。→ 在 `dotenv_override` **之前**快照两个端点变量，
    之后一律用快照值并告警；`.env` 里的 key 与模型名照旧生效（这个项目自己的 key 就
    住在 `.env` 里）。
  - **MED "⎘ Copy recipe" 是同一个陈旧列表形状**，且**反转**了位图蒙版粘贴规则：解码
    途中它把 A 的配方配上 B 的路径，而 `copied_from` 正是"位图蒙版未粘贴"警告的抑制
    条件，于是 A 的栅格蒙版被写进 B 的存档。→ 补 `!self.busy`（与两个同级按钮一致）。
  - **MED 图像端点被套上文本尺寸的上限**：`into_json_capped` 的 64 MiB 是 advisor 的
    文本预算，而它旁边的 SSE 臂给的是 512 MiB——按项目自己的算术，**一帧**全尺寸
    base64 就约 64 MiB，于是已计费的生成被丢弃。→ `into_json_capped_at(r, cap)`，图像
    路径用 `STREAM_CAP`（提为模块级常量，两臂共享）。
  - **MED `into_json_capped` 吞掉 `TimedOut`**：ureq 2.12.1 特意保留该 kind、其余才映射
    成 `InvalidData`，而调用方正是按 kind 二分"端点回了读不懂的 JSON" / "请求已被接受
    (2xx) 可能已计费"。全映射成 `InvalidData` 让**警告花钱的那条分支变成死码**。
  - **本轮自己引入的三条**：`read_sidecar_checked` 先按**字节**读再验 UTF-8（否则 16 MiB
    切在多字节字符中间时，`read_to_string` 先报 InvalidData，用户看到的理由是"不是可读的
    UTF-8"——在一个本轮就是为了"让提示说真话"而做的功能里）；进度门的错误文案不再断言
    "服务器在拖延而非工作"（代理在前面发 `: processing` 时那是假话）；Analyze 的 ✕ 会清
    `busy` 而 `produce_recipe` 根本不收 cancel flag，于是 ✕→Analyze→✕→Analyze 可**叠加
    N 个并发计费调用** → 新增 `analyze_inflight` 闸门，并把两条用户可见文案改成实话
    （"生成类调用会在下一检查点停止；AI 分析会一直跑到完成或超时，仍然计费"）。
  - **LOW**：`serve.rs` 第二处 `is_noop` 仍用夹紧后配方（提示会说"仅因持久化母版才绕过
    清除分支"，而实际是因为带了裁剪）；`fit.rs` 两段**互相矛盾**的注释叠在一起，且
    `render.rs` 把其中假的那条写成不变量（"两个调用方都会应用"——实际只有一个）；
    `write_xmp*` 三个入口没有 `write_recipe` 那道 clamp 地板（字符串上限只结构性保护了
    recipe.json，XMP 靠约定）；`pipeline.rs` 一段四行注释重复了两遍。

  **③ 明确不做（记账，非隐瞒）**
  - **色调限幅在高曝光下的逃生口**——真洞。**这一条我先后判错过两次，最终以真引擎
    实测定案，过程记在这里以免重蹈**：
    - 第一次：我用 Python 复制品测得"`ev+2.0/highlights-100` 加不加限幅器逐位相同"，
      据此**否决了子代理"这是真缺陷"的判定**。错了——复制品与真引擎不同（真引擎先建
      LUT 再线性插值采样），且我当时的度量把"曝光自己压平的区间"整段豁免掉了。
    - 第二次：我改用"非裁切码"度量后，得出 `ev+0.5/contrast+100` 塌 161/4096。也错了——
      那 161 个输入的输出码是 **65535，是裁切**，不是毁细节；而同一个度量反而把
      v0.18.0 修掉的 `highlights +60`（740 段纯白）判成无害。
    - **定案（真引擎，18 配方 × 15 曝光 = 270 格，只数内部码塌陷）**：带限幅器
      **13/270 超过 96、最坏 197**（`contrast -100` @ `+1.5EV`，码 56304）；去掉限幅器
      **95/270 超标、最坏 317**。所以**限幅器是大幅真收益，同时确实留有真残留**——
      两件事同时成立。子代理报的 `ev+2.0/highlights-100` 在真引擎复现为 **145 段、码
      61471**，与它报的码完全一致。
    - 残留全是同一形状：**高正曝光 + 强负向滑杆**。曝光已把顶部节点钉在 1.0，
      `base_gap <= 1e-6` 跳过该区间，滑杆倒置节点对，后备压平。
    - **四个候选修法全部实测否决**：①按滑杆分别限幅（13 格→9 格，但最坏 197→**317**）
      ②最小斜率修复（ev=0 只剩 50% 逐位不变，且 476/1513 改动例**更差**）③带箱约束的
      保序回归（同上）④端点保持式修复（网格最坏 166→210，ev=0 有 300+ 例变差）。
      根因是 KEEP 只约束节点间距、是个代理量——Fritsch–Carlson 在节点已分开时仍可能
      压平跨段。**真修法在色调模型层，且必须视觉验收**，而本轮不允许启动 GUI。
    - 已把该性质按正确度量钉成测试 `no_slider_collapses_an_interior_band_at_any_exposure`：
      `|ev| ≤ 1.0` 守 128（实测最坏 100），`ev > 1.0` 的残留**明写并钉住上界 220**，
      不用放宽阈值掩盖；变异实证 M12（关掉限幅器 → 立即报出 −3EV/blacks+60 的 253 段）。
  - **一个 λ 缩放全部五个滑杆**（子代理测得 ev=0 时 76% 的普通组合被缩放）——与上一条
    同源，同一次改动里解决。

  **④ 第三个独立复审（Opus 顶替额度用尽的 Codex）又找到 6 条，其中一条 HIGH 是
  我这一轮的修复只修了一半**
  - **HIGH `.env` 守卫只挡了 2 个端点变量，漏掉指向【可执行文件】的那几个**——
    `AUTOSHOP_CLAUDE_BIN` 与 `AUTOSHOP_PYTHON` 直通 `Command::new`
    （`advisor/claude.rs:53`、`denoise.rs:186`、`segment.rs:49`，实查确认），两个
    script 变量成为其 argv。于是**我自己写下的那句威胁模型**（"把 .env 丢在共享照片
    包里，解压后就地运行"）仍然成立，而且收益从"改端点"升级成**任意进程执行**，
    严格更糟。`autoshop.local.json` 够不到这些字段（`LocalSettings` 没有），所以
    `.env` 是唯一入口。→ 保护集合改为"决定**去向**或决定**执行什么**"的全部 6 个
    变量；新增测试直接读本文件源码，禁止这 6 个再走 `nonempty()`（变异实证 M11：
    把 `AUTOSHOP_PYTHON` 退回 `nonempty` → 测试精确报出该变量名）。
  - **MED 进度门每字节读一次时钟**：实测 16 MiB 单行 7.57 ms → **382.50 ms（×50）**、
    256 MiB 156.92 ms → **6.23 s**，全部开销就是 `Instant::now()`（22.4 ns/次）；且
    `budget: None` 这个"关闭"开关只关掉了检查、没关掉开销。→ 改为**每次 `read()` 只
    重新计时一次**（8 KiB 粒度 vs 分钟级预算，信息零损失），`None` 时直接返回。
  - **MED `insert_crs_description` 会相信注释/CDATA/PI**：本文件已有测试
    `a_pathological_sidecar_neither_hangs_nor_believes_a_comment` 专门钉这条不变量，
    而我新写的 `find("<rdf:RDF")` 把它**退化**了——头部注释里引用了根标签的边车，会
    把设置块拼进**注释内部**，`merge` 返回 `Some` 于是**不报任何损失**，Lightroom
    读到的文件里一条 develop 都没有。→ 新增 `find_outside_constructs`（复用本文件
    既有的 `CONSTRUCTS`，前进式单趟），三种构造各一条断言，变异实证 M10。
  - **MED ✕ 之后两个 Analyze 按钮显示可点却静默无效**（`analyze_inflight` 闸门只
    加在 `start_analyze` 里，按钮的 `ready` 没跟上），最长 600 秒，期间状态栏还写着
    "应用已解锁"。→ `ready` 纳入 `analyze_inflight`，并给禁用态加悬停解释。
  - **MED README 那句"暖侧 ≤0.25%"是错的**——**我自己犯了"证据范围≠断言范围"**：
    只测了 3200K 就外推。实测 2000K（色温滑杆的下限）处蓝增益是 **−4.43%**，2500K
    是 −0.67%。根因是蓝分支在 1900K 过零，滑杆下限处相对误差被放大。→ README 改成
    完整表格，两端都标出来。
  - **LOW 超限边车提示的措辞与自家测试互相矛盾**（说"它携带的属性不在新文件里"，而
    同一个测试断言 `crs:Texture` 确实在）→ 改成"本次保存改用 Autoshop 自己的上一版
    做底稿——该边车此后的改动不在新文件里"。
  - 复审同时**确认无误**的：`choose_boundary`（20 万次随机反例搜索 0 命中，并证明
    `BOUNDARY` 无非平凡 border 故不可能重叠漏计）、`recipe_to_xmp` 重构后**逐字节
    不变**、保存两次不叠出第二个 Description、clamp 地板不改变正常保存、迁移路径不
    丢 key、快照非竞态。

  **门禁**：218 lib + 1 CLI + 28 GUI 双配置绿、clippy 双零、i18n 459/487/0/0、
  死 pub 0、克隆组 25→25（**与 `104a95d` 同口径实测**：用 worktree 检出旧提交跑同一
  脚本；台账里旧的"74"是另一套口径，不可跨轮比较）、web JS 零改动、密钥扫描零命中。
  测试名按**集合**比对：删 0 / 增 7 / 留 243（新增=keep-alive 进度门、设置保存不洗白
  ambient 端点、**每个指向程序或端点的变量都必须 ambient-unsafe**、boundary 不可伪造
  且线性、超限边车必须披露、merge 提示只为真损失且指对文件、6600K 接缝连续且带内有
  温度信号）。变异实证共 11 个（M1–M11），每条修复至少一个。
  **未验证**：①全程未启动 GUI（用户硬指令），桌面侧只到编译+测试级；②kelvin 重定标
  **改变渲染**且无视觉验收——实测冷侧（>6600K）WB 增益变 +3.19%R/+2.35%B，暖侧
  ~~≤0.25%~~（←2026-08-20 勘误：仅 ≳3200 K 成立——2000 K 蓝增益实为 −4.43%、
  2500 K −0.67%；README 增益表五行经目检包用现行系数+Tanner-Helland 原版独立复算
  逐行复现，README 对、本行错），红通道 6500K 及以下不变；③**Codex 独立复审未执行**——实查真因是**订阅额度
  用尽**（`You've hit your usage limit… try again at Aug 9th, 2026 7:42 PM`），非工具
  故障；按用户当场授权改用 Opus 子代理做反向攻击式复审顶替，结论并入本条台账。

- **第五轮：五代理分区攻击审计 → v0.18.0（2026-08-06，用户令"继续，下一轮"）**——
  按第四轮留下的待办（"下一轮应继续用攻击视角审本轮修复提交自身"）开工，并把上一轮
  子代理**没够到的面**一并分掉：5 个独立子代理分别攻 ①修复提交 `40a1bdc` 自身、
  ②`gui.rs`（13082 行，前四轮无人读过）、③`render.rs` + `fit.rs`/`fit_zoned.rs`
  数值核、④`store.rs`/`config.rs`/`main.rs`/`recipe.rs` 持久化层、⑤`advisor/*` +
  `generative.rs` 网络层。**结论：上一轮的修复本身就是本轮两个要害的来源**——自审
  发现不了的东西，独立审计能发现；而"修完就对"同样是幻觉。

  - **HIGH·上一轮的"O(n) 重写"只是换了个形状继续 Θ(n²)**（xmp.rs
    `skip_text_construct`）：闭合游标缓存确实把**嵌套**形状修好了，但新加的构造跳过
    每轮都重扫整个窗口（`window.find("<rdf:Description")` + 三次 `find(open)`），
    而 `from` 每轮只前进一个构造。**实测（release 复制品，本机）**：

    | 形状 | 修复前 | 第四轮后 | 本轮后 |
    |---|---|---|---|
    | 2.8 MB 嵌套 | 55.97 s | 51 ms | 51 ms |
    | 640 KB 注释 | 51 µs | **8.47 s** | 线性 |
    | 400 KB PI | 90 µs | **9.59 s** | 线性 |

    即第四轮在修一个 DoS 的同时，按字节算引入了一个**更狠**的（16.5 万倍 / 10.7 万
    倍倒退），且干净的二次律（4× 字节 → 16× 时间）外推到新的 16 MiB 边车上限约
    **1.6 小时**，跑在 `SAVE_LOCK` 内并占着一个请求许可，**选中照片**即可触发。
    修：`Landmarks` 把 close/open/三个构造全部做成"只前进"的游标，**跑空的游标永不
    重搜**（这条是关键——对不存在的模式每轮重扫到文件尾本身就是二次律）。
    新测试 `the_scope_scanner_is_linear_on_both_pathological_shapes` 三形状全覆盖，
    变异实证：把构造游标改回每轮重算 → 测试从 0.13 s 变成跑过 60 s 被杀。
  - **HIGH·渲染引擎：普通滑杆值整段抹平色调**（render.rs `build_tone_lut`）。八个
    节点间距只有 0.08–0.25，滑杆偏移直接相加且**从不检查偏移放不放得下**；越界后
    的"修复"是 `prev + 1e-4`，Fritsch–Carlson 随即把两端切线归零，于是整段**精确
    平坦**；顶端则由 clamp 到 1.0 造成同样后果。**实测（16-bit 灰阶经真实导出路径）**：
    `whites: -50` 把输入 0.9568–0.9731 压成同一个码、高光十档从 411 个不同码降到
    75；`highlights: +60` 把 0.8195 以上（18% 动态范围）全部压成纯白（4096 采样里
    740 个连续输入同值）。这两个都是**日常修图值**，不是滥用。
    **四轮测试没抓到的原因**：`region_tones_pin_endpoints_and_stay_monotonic` 只断言
    单调 + 端点不动，而**平带既单调又保端点**——断言选错了不变量。
    修：新增 `render::limit_tone_sliders`，把整个滑杆向量按最大可行 λ≤1 缩放，使
    每个区间至少保住自身间隔的 1%（`KEEP`）；**λ=1 时逐位不变**，所以阈值以内的
    编辑渲染结果完全没变，只有原本正在被摧毁的区间改变（滑杆变成"用尽权限"而不是
    "抹平细节"）。新测试 `no_slider_setting_collapses_a_tonal_band` 覆盖 12 组配方，
    变异实证：去掉限幅 → `highlights +60` 报 740 连续输入同值（与子代理独立测得的
    740 完全一致）。
    - **调参有代价，记录在案**：`KEEP` 初设 0.05 时会**扰动反推**——实测普通反推结果
      （contrast +40.4 / shadows −42.8）在 0.10–0.25 区间 λ=0.990，0.34% 的扰动就把
      `fit` 从"比中性好 3%"（0.08625 vs 0.08918）推过 `err_before`，触发饱和度
      do-no-harm 循环，最终 0.1286（**比不修还差**）。降到 0.01 后同配方 λ=1.03 不
      受限，而 `highlights +60`/`whites -50` 仍被限住。**反推侧刻意不调用限幅器**：
      它用**真实渲染**打分，本来就量到引擎的实际行为，无须预测限幅。
  - **MED·上一轮新加的 clamp 会让"保存"变成"删除"**（serve.rs `api_xmp`）。原注释
    断言"clamping here cannot flip the is_noop branch below"，**是错的**：
    `EditRecipe` 是 `#[serde(default)]`，且 clamp **丢弃整个部件**而不只是夹值——
    零面积裁剪被置 `None`。于是
    `POST /api/xmp {"recipe":{"crop":{"left":0.5,"top":0.5,"right":0.5,"bottom":0.5}}}`
    clamp 后**恰好等于 `EditRecipe::default()`**，落进中性分支 → `clear_develop`
    删掉 recipe.json / XMP / 旧边车，并回 "cleared — saved edits removed"。
    **加 clamp 之前同一请求走的是保存路径**，所以这是上一轮引入的破坏性回归。
    修：清除分支改用**夹紧前**的配方判定（`client_asked_to_clear`），clamp 只管持久化。
  - **HIGH·GUI 版本面板会删掉另一张照片的快照**（gui.rs）。`open_path` 立刻把
    `src_path` 指向新照片，但 `self.versions` 只在**解码完成**的 `Msg::Opened` 里刷新，
    面板也没有任何 busy 门。RAW 解码是数秒级：这段时间里面板显示 A 的 v1/v2/v3、按钮
    可点，而 `delete_version(&src, n)` 取的 `src` 已是 B → **永久删除 B 的快照**
    （recipe + 冻结蒙版栅格，无备份无撤销），随后列表按 B 刷新、状态栏报"已删除"，
    把真相盖掉。"＋保存为版本"是镜像故障：把 A 的画布写成 B 的下一个版本。
    修：`open_path` 在改 `src_path` 的同一处清空 `versions`（列表描述的就是那个键），
    并给整节加 `add_enabled_ui(!self.busy)`。
  - **HIGH·工作目录里的设置文件能改写 API key 的去向**（config.rs）。`serve.rs` 早就
    把"改 base URL → 下次 Analyze 泄 key"当成 HIGH 并加了跨域门，但**文件系统那条路
    是敞开的**：解析是**按字段**而非按文件，`pick_opt(image_api_key, env)` 在文件没给
    key 时回落到环境变量，`pick(image_base_url, …)` 却让文件赢。于是一个只写
    `image_base_url` 的 `autoshop.local.json` 就能把真 key 送到攻击者端点——解压别人
    的照片包、在里面运行 Autoshop 即可。**且本机当前生效配置正是这个形状**（实查：
    `image_api_key` 为空、`image_base_url` 由 cwd 文件提供）。
    修：`SettingsOrigin` 区分中央件与 cwd 件，cwd 件只保留模型/供应商选择，
    **key 与 base URL 一律忽略并告警**。用户现有配置的两个 base URL 恰等于默认值，
    因此**行为零变化**（已实查确认）。
  - **MED·`clamp()` 一个字符串都不夹，注释却说"每个字段都有界"**（recipe.rs）。
    `rationale` 由 advisor 失败文本填充，而上游 HTTP 错误体会**原样**流进去
    （`AdvisorError::Http{body}` → `fallback_reason` → 启发式 rationale），所以
    一个**只是坏掉**的端点就能把数 MB 写进 recipe.json **和 RAW 旁的 XMP**；超过
    16 MiB 后 `read_sidecar` 拒读，**又会静默让 Lightroom merge 失去底稿**。
    修：`rationale`/蒙版 `name`/`Bitmap{path}` 分别加 4096/256/4096 上限，按字符
    边界截断（`String::truncate` 越界会 panic，而这是无人校验的输入）。
  - **MED·两处无上限的响应读取**（advisor/mod.rs、generative.rs）。SSE 两条臂都带
    上限（64 MiB / 512 MiB），**阻塞 JSON 两条臂却直接 `into_json()`**——ureq 自己的
    文档就写着 `into_reader` 无界、"应当使用 `.take()`"。Rust 分配失败是 **abort**，
    即桌面端连同未保存工作一起硬崩。修：`advisor::into_json_capped` 统一加 64 MiB 顶。
  - **MED·一个文件两个 develop 目录**（store.rs `photo_key`）。键是 `<stem>-<hash>`，
    但 **hash 取自 `absolute(src)`、stem 取自原始 `src`**。Windows 打开时会丢尾点，
    所以 `…\DSC001.NEF` 与 `…\DSC001.NEF.` 是同一个文件、hash 逐字节相同，
    `file_stem()` 却分别给出 "DSC001" 与 "DSC001.NEF" → 两个目录，一边存的编辑在
    另一边**完全不可见**。修：两半都取自 `abs`；普通路径两者一致，故**不重键**任何
    既有存储。
  - **LOW·上一轮那条"钉了 11 种权威串"的测试，有一条钉了个寂寞**（serve.rs
    `loopback`）。`"[::1]".rsplit_once(':')` 会在 IPv6 字面量**自己的**冒号处切开，
    得到 name=`"[:"`，所以 `assert!(!loopback("[::1]", 8080))` 是因为**名字没匹配**
    才过的，不是因为端口规则——把端口修复回退掉它照样绿。修：先拆方括号再找端口，
    并补上**正向**断言 `loopback("[::1]", 80)`（回退端口修复即失败）；顺带按 RFC 3986
    让主机名大小写不敏感。
  - **LOW·许可测试同样没钉住"唤醒"**（serve.rs `Permit`）。整个测试都在 `max = 8`
    下且一次只持一个许可，**没有任何线程进过 `cv.wait`**——把 Drop 里的
    `cv.notify_one()` 删掉，测试全绿，而真实服务器在 8 次 panic 后永久卡死（正是这
    条测试命名要防的那个 bug）。修：补一段 `max = 1` 的等待者场景，变异实证：删掉
    notify → "the waiter was never woken" 超时失败。
  - **本轮明确未做（下一轮的账）**：`pipeline.rs` 的 merge 失败提示有两个问题——
    ①对 0 字节或**非 Camera-Raw**边车（exiftool / Bridge / Capture One 的评分关键词
    XMP）会**每次保存都误报**，且消息里插的是 `out.display()`（store 里的副本），
    指向的是一个**根本没被动过**的文件；②`read_sidecar` 超 16 MiB 返回 `None` 后，
    `.or_else` 会**换成我们自己上一次的投影**当底稿，merge 成功 → **真丢属性时反而
    一声不吭**（正是上一轮加提示要防的那个失败模式，从旁门绕回来了）。另有：流式
    路径无总时长上限（keep-alive 字节可让 Analyze 永久挂住且 `start_analyze` 未
    `arm_cancel`）、`decode.rs` 两条 `file:line` 引用指错函数、`part_text` 多部分
    注入（LOW，两个代理都判为非越权）、`kelvin_to_rgb` 在 6600 K 处不连续（绿通道
    跳 1.32%，且 6600–6688 K 有 88 K 的色温死区）。
  - 门禁：**211 lib + 1 cli + 28 gui** 双配置绿、clippy 双零、i18n 458/486/0/0、
    web JS `node --check` 通过且与上轮逐字节相同、死 pub 项 0（275→280，即本轮新增
    的 5 个公开项）、克隆组 74（与上轮相同）。测试名按**集合**比对：**删 0 / 增 5 /
    留 238**。每条修复都有具名变异实证（构造游标不缓存、限幅器去掉、rationale 不夹、
    stem 取原始 src、cwd 件保留 base URL、Drop 不 notify）。
  - **未验证**：全程**未启动 GUI**（用户硬指令），桌面侧只到编译+测试级；色调限幅
    虽逐位证明"λ=1 时不变"，但**未做视觉验收**——建议用户拿一张高光丰富的片子对比
    `whites -50` / `highlights +60` 的前后表现。

- **第四轮：攻击视角子代理审计 → v0.17.0（2026-08-06，用户令"派遣一个子代理用攻
  击视角全面审阅并debug"）**——首次由**独立子代理以攻击者立场**扫全仓（此前三轮
  都是自审）。返回 8 条，**我逐条对码复核，8 条全部属实，零证伪**。
  - **HIGH·跨域门的无端口空洞**（serve.rs `loopback`）：`Origin: http://localhost`
    没有冒号 → 端口被当作**通配**而非"方案默认 80"，于是服务在 8080 时任何
    loopback:80 上的页面（本机 IIS/XAMPP、docker -p 80、有 HTML 注入的文档服务器）
    都算同源，可发简单请求 POST `/api/settings` 把 AI base URL 改到攻击者，
    **下一次 Analyze 就把用户的 OpenAI key 送出去**。修：缺省端口即 80；
    `loopback` 提到模块级以便直测；新测试盖 11 种权威串。
  - **HIGH·请求许可在 panic 时永久泄漏**（serve.rs 请求闸门）：许可由 `handle`
    之后的直线代码归还，panic 展开会跳过它 → 8 次 panic 后 accept 循环永远卡在
    `cv.wait`，**服务器彻底不再应答**，用户只看到浏览器转圈。该文件其它每个
    `Mutex::lock` 都写了 `unwrap_or_else(|p| p.into_inner())`（即已假设 handler 会
    panic），唯独闸门没贯彻。修：RAII `Permit`，Drop 里归还且**不**按
    `thread::panicking()` 短路；新测试用真 panic 线程验证槽位归还。
  - **MED·XMP 扫描器两个真缺陷**（xmp.rs `find_matching_close`）：①闭合标签搜索
    每轮从头重扫 → 对嵌套深度 Θ(k²)，**实测 20000 层嵌套 13.62s vs 修后 0.02s
    （680×）**，且这段跑在 `SAVE_LOCK` 内并占着一个请求许可；②不跳过注释/CDATA/PI，
    于是注释里的 `</rdf:Description>` 被当成真闭合（它的两个兄弟函数早就跳了）→
    body 被截断 → 整个 merge 退回重新生成，**把它本该保护的 LR 属性全丢了**。
    修：闭合游标跨轮缓存（等价性有证明：`from` 只前进，故区间内无闭合）+ 新增
    `skip_text_construct` 三构造跳过（未闭合构造 → 整档回退，绝不猜）。
  - **MED·唯一持久化路由不做 clamp**（serve.rs `api_xmp`）：render 三个路由都写了
    "untrusted network input" 并 clamp，**写盘的那个没有**。`EditRecipe::clamp`
    的注释恰恰点名"a hostile POST to the local web server"是这些上限存在的理由。
    10 万个蒙版的 body 就成了该照片权威的 recipe.json，之后每次打开都要重新解析，
    还会原样进版本快照。修：api_xmp 补 clamp，**并把上限下沉到持久化边界**
    （`write_recipe` clamp 它写出的那份克隆，调用方内存不动）——未来任何新路由
    忘了 clamp 也踩不穿这层地板；新测试 1000→64 且调用方仍是 1000。
  - **MED·XMP merge 失败静默降级**（pipeline.rs `write_xmp_doc`）：merge 不成即
    重新生成整档，`crs:Texture`/相机配置文件与 Look/LR 镜头块/外来 ns/xpacket
    **全部丢弃且零披露**——正是 A11 当初要根治的数据损失，从后门又走了回来。
    修：`write_xmp_doc` 返回 `(路径, Option<note>)`，stderr 必报，web 保存回执与
    GUI 保存状态/toast 都带上；`write_xmp` 保持原签名（一份实现两个入口，
    同 decode.rs 的 `preview_only`/`embedded_preview` 惯例）。
  - **LOW×3**：`header()` 放行 CR/LF（tiny_http 的 `AsciiString::from_ascii` 只挡
    非 ASCII，源码已核）——当前无可控值故不可利用，但保证只写在每个调用点的记性
    里，改为构造器拒绝控制字节并订正该函数的文档（它原本把危险说成"非 ASCII
    panic"）；`html_response` 补 `X-Frame-Options: DENY`（端口固定，框架点击劫持
    对同源门免疫）；五处 sidecar `read_to_string` 无上限 → 统一走新增的
    `store::read_sidecar`（16 MiB 上限，`Read::take` 而非 `metadata()`，避免
    stat-then-read 之间文件变大）。
  - **审计中唯一未采纳的一条**：客户端可给 `Bitmap` 蒙版传**绝对路径**，
    `resolve_mask_paths` 只重锚相对路径（store.rs），理论上能把任意本地图片的亮度
    渲进成品。判定：修好跨域门后网络侧入口已关闭；而把非 store 的绝对路径改为
    inert 会让手写 recipe 的合法用户**导出直接被拒**（引擎既有"不可读栅格→成品
    拒绝"契约）。**如实记录为已审查未改**，不是遗漏。
  - 子代理另提"ARCHITECTURE 说 201 个库测试，我数到 204 个 `#[test]`"——用
    `cargo test` 实测澄清：204 个总数 = 201 通过 + 3 `#[ignore]`，文档正确，非缺陷。
  - 门禁：**206 lib + 1 cli + 28 gui** 双配置绿、clippy 双零、i18n 458/486/0/0、
    web `node --check` 通过；`pub` 死代码 0（273→275，新增两个公开入口）；克隆组
    73→74（+1，如实记）；测试名集合 diff 删 0 增 5 留 233。
  - **收敛判定（如实）**：本轮**非空且找到了前三轮都没找到的两个 HIGH** ——
    自审三轮的盲区由攻击视角一次打穿，这本身是本轮最重要的结论。**下一轮应继续
    用攻击视角审本轮的修复提交自身。**

- **第三轮 debug：收敛确认 + 全量文档对齐（2026-08-06，用户令"第三轮 + 确保所
  有文档同步更新，对齐事实"）**——审查对象=第二轮的提交 `5aa9d28` 自身。
  - **生产行为轴：连续第二轮零发现。** `5aa9d28` 的五条新测试逐条重读重跑，断
    言与被测契约一致。`apply_crop` 注释里"零尺寸帧会撞上下游 `par_chunks_mut(0)`"
    经核实为真：render.rs:1231/1278/1645/2629 四处按行分块都没有零宽守卫，全仓
    唯一有守卫的是 U14 补进 `apply_radial_gain`（render.rs:1029）的那处——所以
    `apply_crop` 拒绝生成零尺寸帧确实是这条链上的真门。
  - **记录轴：两条注释在撒谎，已订正**（本项目一贯把假注释当缺陷处理）：
    - decode 测试称"**每个** `embedded_preview` 调用方都对 `Err` 打印诊断"——实
      为三个调用方，其中 GUI 开图（gui.rs:2160 `_ => Vec::new()`）**故意**把
      `Err` 与 `Ok(None)` 合并（开图不因缺相机渲染而失败）。改为点名"三取其二"
      并写明 GUI 这条例外。
    - 同一测试称非 RAW 的 `Ok(None)`"是基调估计器所依赖的"——三个调用方
      （pipeline.rs:637 / serve.rs:706 / gui.rs:2098）全都先自行 `is_raw` 短路，
      生产路径根本到不了该分支。改为如实陈述：钉它是为了让这条保证是**契约**，
      而不是下一个调用方无权依赖的巧合。
  - **重复轴：一处真实的契约重复。** `/api/fresh-base` 与 `/api/recipe` 的 404
    体各自手搓同一份四键标定 JSON（serve.rs:914/956，8 行 ×2），客户端也各读一
    遍（index.html:999/1021）。任一端加键而漏改另一端 = 同一张照片的标定取决于
    客户端走了哪扇门。服务端提取 `fresh_base_payload`、客户端提取 `takeFresh`，
    并补契约测试钉死键集（变异 `as_shot_k`→`as_shot_kelvin` 实证被杀）。
  - **文档全量对齐**（用户令"确保所有文档同步更新，对齐事实"）：
    - README **整个桌面 GUI 只字未提**——`src/bin/gui.rs` 13069 行（全仓最大文件、
      约占三分之一代码），每个 tag 都随发布上传 `autoshop-gui.exe`（v0.16.1 实测
      35475001 字节），而 README 的特性表、快速上手、Tech 三处都只有 CLI 与 web。
      已补：特性条目、`cargo build --release --features gui --bin autoshop-gui`
      构建行 + 为何藏在 feature 后（Cargo.toml:25-31）、Tech 补 `eframe`/`egui`
      与 `rayon`。
    - README **整条 `match` 子命令缺失**（main.rs:169-196）——`src/fit.rs` 1568 行
      + `src/fit_zoned.rs` 931 行、GUI 也在用（gui.rs:9987）的全仓第四大子系统，
      命令表和特性表都没有它。已补两处。
    - ARCHITECTURE 组件表同样没有反推行，§4.1–4.7 也无对应小节 → 新增表行 +
      **§4.8 反推**，按 `fit.rs` 模块文档如实写（分布级而非逐像素回归、三阶段
      顺序、三道否决、`--zoned` 与"XMP 只带全局拟合"的边界）。
    - ARCHITECTURE §6 遗留问题 #5/#6 还写着"待确认 / research underway"——两者在
      出货代码里早已落定，改为已解决并附 `file` 引用；§4.2 的 M1 `[verify]` 同步
      改为"已由出货代码落定"（Responses API + 严格 `json_schema` + base64
      `input_image`，模型 id 走配置）。
    - M1_PLAN / V2_PLAN 已在 `a4f818f` 加过历史文档横幅，本轮复核无新漂移。
  - 门禁：**201 lib + 1 cli + 28 gui** 双配置绿、clippy 双零、i18n 458/486/0/0、
    web `node --check` 通过；克隆组 74→**73**、`pub` 死代码 0；测试名集合 diff
    （对 `5aa9d28`）删 0 增 1 留 232。
  - **收敛判定（如实）**：生产**行为**轴连续两轮零发现=已收敛；但本轮**仍非空
    轮**（2 条假注释 + 1 处契约重复 + 4 处文档缺口，其中"README 从未提过桌面
    GUI"是全仓最大的一处文实不符），按本项目自己的规矩，宣布完全收敛仍需下一轮
    归零。**下次开工可直接跑第四轮。**
  - 本轮改了生产代码（两处等价提取），但**行为未变**，且已发布的 v0.16.1 二进制
    与之无关——未发新版。

- **第二轮 debug：对 v0.16.1 修复提交自身的复审（2026-08-06，用户令"再进行
  一轮debug，确保收敛"）**——按本项目自己的规矩，先审查**上一轮的修复提交
  本身**（历次战役 21 条自伤回归都是这样抓回来的）。
  - **缺陷轴：零发现。** 逐块重读 `db2509a` 的全部 hunk，七处提取逐行核对
    为等价。两处经查证伪的疑点如实记录：`eval.rs:45` 与 `style.rs:114` 表面
    仍读未加 scope 的 `xmp`，读码后确认上方均有 `crs_own_scope` 遮蔽绑定；
    `apply_radial_gain` 把零尺寸守卫移进共享函数后会先建 LUT 再返回，查
    `profile_knot_interp`（`n == 0` 返回 1.0）且调用点由 `vignette_active()`
    把关，空 knots 到不了。
  - **测试轴：五个空洞，全部由该重构造成或暴露**，逐条补齐：
    - `render::apply_crop` —— 这处提取的**全部理由**就是"RAW 与烘焙两条路径
      必须一致"，而它一条测试都没有。新测试钉死精确矩形（尺寸取自 extent 而
      非 right/bottom 边、原点不可换轴）并经 `render_to_file` 走真实烘焙路径
      端到端核对交付尺寸。
    - `decode.rs` **全文件此前无测试模块**（全仓唯一无单元覆盖的核心解码
      层）。新测试钉死 `camera_rendition` 两调用方仅有的两处差异：非 RAW 时
      `preview_only` 载图而 `embedded_preview` 返回 `Ok(None)`；**无法解码的
      RAW 两者都必须 `Err`**（生产调用方对 `Err` 打印诊断、对 `Ok(None)` 静
      默，合并二者会把坏文件藏进静默跳过）。
    - `serve::deliverable_source` —— A6「成品不再撒谎」的拒绝语义（死母版链
      500、伪造声明 400）在两个 handler 合并后失去覆盖。
    - `gui::release_empty_claim` —— 松弛变异会把"归还预留的名字"变成"删除用户
      的部分修饰结果"。
    - `gui::sample_5x5_mean` —— 平均与点采样无人区分。
  - **每条新测试都用变异实证**（施加具名变异 → 确认失败 → 还原 → 确认通过），
    共 7 个变异体全部被杀：裁剪尺寸取自边/裁剪原点换轴/解码失败变静默 None/
    丢失非 RAW 守卫/死母版链退回原图/删除非空文件/点采样。
  - clippy 抓到本轮自己的失误：decode 测试模块置于 `impl Decoded` 之前
    （`items after a test module`），已移到文件末尾。
  - 门禁：**200 lib + 1 cli + 28 gui** 双配置绿、clippy 双零、i18n
    458/486/0/0、web `node --check` 通过；克隆组 74 未增、`pub` 死代码 0、
    死代码抑制标记 0。测试名**集合** diff（对已发布的 `v0.16.1` 取基线）：
    删 **0**、增 **5**、留 227。
  - **生产代码逐字节未动**（`202 insertions(+), 0 deletions(-)` 全为测试），
    故已发布的 v0.16.1 二进制不受影响，本轮不发版。
  - **收敛判定（如实）**：缺陷轴本轮零发现＝已收敛；但本轮**不是空轮**（5 项
    覆盖发现），按本项目的收敛规则，宣布完全收敛需下一轮归零。

- **单通道 debug（2026-08-06，用户令"进行一次完整的单通道debug"）**——单人
  单轨全仓复核，基线门禁全绿后系统性通读 + 定向扫描，四条落地：
  - **XMP 读取端穿透嵌套 Look（真缺陷，实证）**：写入端自批 3 起就是深度
    感知的（`top_level_owned_spans`，为的正是 Adobe 把配置文件烘焙参数写成
    `<crs:Look><rdf:Description><crs:Parameters><rdf:Description><crs:…>`
    这一形状），**读取端 `crs_str` 仍是全文扁平扫描**——顶层 Description
    缺某个键时，导入会从 Look 里取值当成用户滑杆。探针实证：clarity 0→50、
    vibrance 0→35、还会学走 Look 烘焙的色调曲线。根治=新增
    `xmp::crs_own_scope`（顶层标签 + 只属于它的子元素；嵌 `rdf:Description`
    的容器一律丢弃，`MaskGroupBasedCorrections` 按名保留因其嵌的是自己的
    蒙版项；无法核算的标记回退整档=修复前行为）。四个整档读者全部改走
    scope：`xmp_to_recipe`、`unparsable_crs_numbers`（披露必须与导入同步）、
    `style::read_settings` + `eval::user_curve_shape`（否则风格索引把 Adobe
    的 Look 学成"这个用户的偏好"）。两条回归测试钉死。
  - **i18n 死翻译**：`("Temp","色温")` 的调用点早已改为 "Temp (K)"/"Temp
    shift"，条目成孤儿 → 删除（漂移 1→0）。
  - **`apply_wb` 是唯一漏掉的串行逐像素段**：v0.11.0 的 rayon 扫荡覆盖了
    色调/HSL/调色/曲线/去雾/暗角，独漏这一条——而它正是每次 Temp/Tint 改动
    都要跑、导出时按全传感器分辨率跑的那条。改 `par_iter_mut`，逐位不变
    （每像素只读自身 + 只读 LUT，无累加无顺序依赖；由既有的
    `full_frame_local_wb_matches_the_global_wb_stage` 交叉钉死）。
  - **文档数字失实**：ROADMAP 记 v0.16.0 为 51 提交，tag 区间实为 101；
    ARCHITECTURE 的测试数 195 → 197。
  - **测试模块通读的产出**（用户追加令"看到多余/重复/累赘的注释或者代码，
    直接清理干净"）——六处清理，全部为注释/死代码，无行为改动：
    - `style.rs::tag_describes_a_bright_tele_landscape` 只断言首尾两段，
      **从不读时间段分量**——而 fixture 恰恰是刻意设了 `f[3]`/`f[4]` 才能
      定出那一段；改 `assert_eq!` 钉全串 `tele/bright/night/landscape`，
      同时删掉自相矛盾的注释（"midday-ish … → hour 0 = night"）。
    - `pipeline.rs` 里"高调照片"那段注释被历次编辑挤到了 `washed_clipped`
      头上，离它描述的 `high_key` 有 13 行 → 移回本位。
    - `render.rs::dehaze_negative_adds_a_veil_without_clipping` 的
      `let (w, h) = (64, 1)` 配一个结尾的 `let _ = h;` → 直接 `let w = 64`。
    - `store.rs` 函数体内重复 `use crate::recipe::LocalAdjustment;`
      （测试模块顶部已导入）。
    - `i18n.rs` 悬空节注释 `// GRADE_REGIONS labels` 下面一条条目也没有
      （四个标签实际散在 Tone 节与 UX 批次里）→ 删；另两处"某键已删除"的
      墓碑注释 → 删（git 历史即记录）。
  - 测试模块本身的结论：经 R1–R26 与 U14 加固后，断言普遍附带"这一条杀死
    哪个具名变异"的说明，本轮未再发现瞎测试，唯一例外即上面那条 style.rs。
    覆盖缺口如实记录：**`serve.rs` 2374 行只有 4 条单元测试**（HTTP 处理层
    几乎无单元覆盖，靠 lib 层与真机验收兜底），本轮未扩测。
  - **死代码 + 重复清理与等价精简**（用户令"全量清理死代码+多余/重复/累赘
    的注释；能精简的地方进行等价精简"）——先量后改：自写函数长度/复杂度
    度量与克隆检测（scratchpad 的 `fnmetrics.py` / `dupes.py` / `deadpub.py`，
    带 Rust 原始字符串与块注释掩码），再按度量结果动刀：
    - `pub` 死代码扫描：273 个 `pub` 项**无一处只出现在定义处**——lib 的
      公开面全部有调用点，无可删项（如实记录，不是"清理了 N 个"）。
    - `recipe.rs` 的 `#[allow(dead_code)]` 是**过期抑制**：实测删掉后 clippy
      双配置仍为零告警——`clamp`/`is_noop` 早已是 pub 方法，永远不会被判死。
    - 七处等价提取（全部为纯搬移，行为逐位不变）：`render::apply_crop`
      （RAW 与烘焙两条路径的裁剪取整规则合一，二者必须一致）、
      `render::apply_radial_gain`（手动暗角与机内暗角只差 LUT 的构造方式，
      几何/传输对/遍历合并）、`render::rgb16_source`（三处 `let owned;`
      延迟初始化的借用-或-转换惯用法 → 一个 `Cow`）、
      `gui::release_empty_claim`（**五对** worker 失败/panic 尾部，连同逐字
      重复五遍的 7 行注释）、`gui::sample_5x5_mean`（WB 吸管与颜色范围取样）、
      `serve::deliverable_source`（export 与 download 的母版解析 + 纪元修复，
      各 ~30 行逐字相同）、`decode::camera_rendition`（相机自有渲染的三级
      回退，`preview_only` 与 `embedded_preview` 只差"没有算不算错"）、
      `retouch::heal_and_save`（heal 与 clone_stamp 的原深度+alpha+暂存写）。
    - 度量结果：函数总行数 31264 → **31076**，克隆组 83 → **74**。
    - 未做的取舍：`gui.rs` 五个 `start_*` retouch worker 的外层脚手架仍有
      ~16 行相似度，但中段各不相同，再抽象需要高阶闭包参数，收益不抵可读性
      损失；测试夹具的重复（如 `poll_workers` 的两个 AutoshopApp 构造）保留
      ——每条测试自带前提比共享构造器更易读。
  - 门禁：**197 lib + 1 cli + 26 gui** 双配置绿、clippy 双零、i18n
    458 调用点/486 条目/0 未译/0 孤儿、web `node --check` 通过、密钥扫描
    干净。测试名**集合** diff（`git stash` 取真 HEAD 基线后逐名 `comm`）：
    删除 **0**、新增 **2**（即上面那两条 XMP 回归），HEAD 的 225 条全部
    健在——重构未吃掉任何一条测试。
  - 覆盖度如实标注：**全仓 Rust 源码逐行通读完成**——lib / CLI /
    `src/bin/gui.rs`（13059 行）/ `src/web/index.html`（1689 行），
    **含每个文件的 `#[cfg(test)]` 测试模块**、`src/bin/i18n.rs` 的 486 条
    翻译表、`python/{denoise,segment}.py`、`build.rs`。未读：
    `python/weights/*`（vendored 上游权重，二进制）。**本轮未跑 Codex
    独立复审**（本会话禁用子代理）。

- **R12 双路全仓 debug 修复潮（2026-08-04，用户令"你自己和codex进行两路
  全仓debug，收敛后推送，发布新版本"）**——R1 双路扫描（Claude 12 单元
  舰队 + codex 10 单元）132 条原始 → ~93 条去重逐条裁决，七个修复批全部
  落地（台账=会话 scratchpad/r12_master.md）：
  - **批43 `bba24e1`（render 引擎）**：头号伤——v0.15.0 的工作分辨率帽
    `downscale_f32` 走 `image::thumbnail`，其 f32 路径 `(sum+n/2)/n` 的
    取整项对浮点通道**恒 +0.5**：GUI 打开预览/修饰母版/web 预览/
    `photo_base_knots` 基调估计全部偏白（导出 max_edge=None 未中招）。
    先写失败测试（平场 [0.25,0.5,0.75] 出 [0.75,1.0,1.25]）再手写无偏
    box 平均（复刻 resize_dimensions + ceil 窗口语义，f64 累加）。另：
    去雾气光采样 splitmix 哈希逐行相位（批42 剪切网格在 stride 2 锁死
    棋盘格）；apply_masks 双零尺寸守卫；generic-save 从真目标取格式。
  - **批44 `1279ad9`（render 测试）**：十处弱断言致死化——原子性测试从
    未测过原子性（只读目标制造真"编码后发布失败"）、blending 振幅三档
    扫描、晕影/锐化灰进灰出逐通道、预览 WB 逐字节等于共享阶段、HSL 五
    带对照、RGB 三通道曲线、金丝雀自比恒真臂删除、开尔文严格比较、拉直
    0° 逐像素。全部先对真引擎验证（引擎对，测试瞎）。
  - **批45 `5a79644`（store/xmp）**：clear_pixel_source 连 `.bak` 一起删
    （否则 recover_orphan_baks 复活已解绑母版——.bak 先删=崩溃安全序）；
    recover_orphan_baks 改副本暂存 + publish_no_clobber（fs::rename 会
    替换并发新存档；幸存者绝不在未落地前被消耗）；photo_key stem 半边
    Windows 大小写折叠（NTFS 解析使预折叠库仍可达）；merge 补剥属性
    **元素形式** crs（"no known producer" 自相矛盾注释订正）；crs_f32
    拒 NaN/inf 且披露扫描直接调用它（结构性防漂移）。
  - **批46 `609321c`（serve/web）**：**claim 计数器与 installed 代号分
    离** + `at_checked` 在列表锁内一体解析 id（关掉慢扫描窗口与大 body
    窗口两条跨照片写入路径）；**会话母版签发登记表**（photo_key→路径→
    generated 旗标：同 stem 跨目录嫁接与 generated 剥离丢失一并关闭，
    api_xmp 按真实出处记录）；web 客户端九处未保存保护（换目录确认/切
    图即空配方/analyze 基线保 master 半边/Reset 清会话母版/saveXmp 响应
    绑定/存后剪 null-master 历史/盖戳后 markSaved/Ctrl+S 提到打字门前/
    revealNew 代号护栏）；recipe.json 损坏落 XMP 不再无声。
  - **批48 `cbbb695`（pipeline/advisor/generative）**：**任何 2xx 后不再
    重发**（文本 SSE/阻塞体/图像读+解析——图像旧口甚至用了 30s 文本窗
    而非本模块 3s）；find_raws/find_sources 规范身份环去重 + 子目录报错
    跳过（连接环曾使每张 RAW 计费至 64 次）；Refine 失败不再退无基线
    启发式（当前编辑不换掉）；剥离点下沉 produce_recipe + carry_over 带
    非 Default lens_profile（未保存镜头开关不再回退）；蒸馏后复验降级
    保对；auto 先验 -o 再计费；批量渲染失败先落存档（重跑不再重计费）；
    dotenv Once 化；生成输出暂存改名；填充合成期取消检查点；
    ModelFailure 变体；stall 消息报生效预算。M29 边车硬超时维持立项。
  - **批49 `6fac83b`（fit/eval/style）**：match 给拟合配方盖
    photo_calibration 三元组（拟合解在嵌入 JPEG 上、成品却渲染在中性
    显影上——H18+M35 一并根治）；旋转普查 before 侧地板 0.03（0.035
    淡蓝天空重着色曾整体躲过普查；0.015 实测雾霾份额 0.0414=0.83×阈值
    → 数据定标 0.03）；do-no-harm 终态中性回退；eval 0↔0 行不再稀释
    MAE + 无测量打 n/a + 曲线重复输入先者胜 + 单点曲线入库；style tint
    与开尔文成对、非有限样本拒收（NaN→null→整库不可载）、自排除按路径。
  - **批47 `c9aad09`（gui）**：**导航暂存背整条变体带**（StashEntry.
    others + 无头往返测试——后台变体编辑不再随缩略图点击蒸发）；
    **cleared.txt 最新意图标记**（Reset+Ctrl+S 清除后，RAW 旁旧拷贝投影
    不再复活已清编辑；图库只读铁律不破，清除后的 LR 新编辑仍胜出；GUI+
    web 双清除点盖戳）；范围蒙版基准改**前缀**（引擎按顺序堆叠评估——
    全清基准把蒙版2的范围判在蒙版0/1已改过的像素上）；改名缓冲随重排
    remap/随照片与变体边界消亡/Analyze・存版本・拷配方三入口冲刷；裁剪
    无框方向键不再物化恒等框、旋转带只归全幅框、预设随工具消亡、参考线
    按真框计算；\-锁 Before 提示改真话+Esc 可出+随照片复位；双击缩放不
    再吃掉 AI 选区；J 快路径削波层同步裁剪留白；F1 可关速查表；缩略图
    失败可重试；keep 重解码失败消费自身暂存副本；GUI refine 基线不再预
    剥离（承接批48）。
  - 接受/立项残余：单张导出同名覆盖（镜像 GUI 政策，注释落档）、裁剪
    预设一帧 tex_size 陈旧、M29 边车超时、旋转普查 <0.03 色度盲区、
    dotenv 每进程一次、会话母版登记表随进程消亡（reselect 契约）。
  - **R2–R25 二十四轮对抗复审（2026-08-05）**——每轮=舰队多透镜找证
    （每条 finding 交两名对抗验证者独立复核，2 票才算确认）+ 独立第二轨
    （codex 额度耗尽后按用户令改用 opus）。**纪律：每个修复提交本身进
    下一轮审查**——正是这条抓回了修复自己引入的 **21 条编号回归**。批 50–84
    共 35 批全部落地，台账=会话 scratchpad/r12_master.md。主线三条：
    - **纪元修复机制（批 52、59–84）**：批 43 修好采样器后，**旧版存下的
      基调曲线是对着偏白帧拟合的**，在修正后的显影上渲染暗好几档。
      `pipeline::repair_pre_era_base_curve` 按指纹（version<2 + 内部节点
      x≥0.5 + 处处不提亮且至少一处暗过 0.05）重估，三态语义
      （None=本次无能力→不缓存待重试／Some(空)=该照片本就无基调／
      Some(节点)=替换）+ 进程内记忆表（键含**读取前**捕获的文件身份，
      先答者胜）。规则逐轮长成：**凡写者先修**（produce_recipe／match／
      api_xmp／Ctrl+S／Save-all，披露随变更同行）、**凡读者先修**
      （load_active／暂存重试／开图门／apply_step=历史唯一出口）、
      **凡交付物锚定在照片而非烘焙母版**、**先剥离再修复**、
      serve 三端点按 funnel_ran 修复并经 X-Recipe-Warning 告知 web。
    - **母版身份两种拼写（批 79–81）**：会话内修饰记 `./out` **相对**路径，
      `store::write_pixel_source` 存**绝对**路径，而保存不改写内存值——
      裸比较使**已保存的修饰照片永远自称未保存**（●常亮／退出对话框列出／
      Save-all 重写／导航暂存反复武装）。`same_master(_opt)` 词法归一后，
      六处身份比较（暂存门／●／条带脏计数／退出对话框／关窗守卫／历史
      像素身份）收敛到一个函数。
    - **预览分辨率一致性（批 77–84）**：拒绝与失败都不得留下残余（组合框
      已经改过的值必须回滚）、烘焙跟**画布**而非全局偏好、**画布有两个
      合法落点**（偏好=重解码母版的落点／源交付边=小传感器新鲜烘焙的落点，
      只有第三值才算陈旧）、四道装载画布的门（切变体／撤销重做／暂存恢复／
      删除变体）共用一个写入器且**两个方向都写**（分歧出现要说，分歧结束
      要收回）、拒绝按成因分三种措辞且绝不给不可能的补救。
    - 其余要害：批 51 XMP 的 CDATA 不再吃掉用户 sidecar；批 53 未动滑杆的
      生成填充也算编辑；批 54 clear 的另一条复活路径 + 无人能读的警告；
      批 55–56 修复潮自身仍能丢工作的三条 + 无限重试 + 过度承诺的对话框；
      批 57–58 不成立的陈述与在完美拟合上误触发的安全网；批 84 打开失败
      不再留下无主历史（撤销曾对已不存在的照片执行）。
  - **收敛判定（2026-08-05）**：第二十五轮双轨对批 84 复审，**两轨均零确认**。
    终态门禁 **195 lib + 1 cli + 26 gui** 双配置绿、clippy --all-targets
    双零、i18n 0 drifted、web 脚本可解析、密钥扫描干净、测试无 ./out 残留。
  - 接受/立项残余（逐条记于代码或台账）：五个烘焙调用点无无头测试（worker
    起网络调用）；`baked_canvas_edge` 的烘焙栅格闸可证冗余、保留为防御层；
    apply_step 的 open_in_flight 臂自批 84 起不可达、保留（键盘一旦解禁即
    回归）；修复记忆表身份=尺寸+mtime；粘贴路径的 UI 线程估计；
    默认修饰母版 2048px 上限在 4096 偏好下会被整数放大（早于本战役、
    有设计注释）。
  - **v0.16.0 发布**：**101 提交**（`git rev-list --count v0.15.0..v0.16.0`
    = 101，其中 43 条 `R12 batch` + 7 条 round-N 舰队 + R11 尾部批 22–42
    + 文档/发版）。此前记的 51 是漏计——发版范围按 tag 区间核定。
    发布说明披露：v0.15.0 的预览/修饰母版/基调估计整体 +0.5 偏白；该期
    首次打开的照片基调曲线来自偏白帧，本版在每条读写路径上自动重估并
    告知。

- **R11 后修复潮一（2026-08-04，用户四项拍板后开工）**——四项决策：成品
  重名=批内避让+披露、非 Accept 判词不自动保存、宽色域=全链路根治、
  WB=绝对开尔文（待做，设计已入台账）。落地六批（22-27，全部未推送）：
  - **批22 `b02c28e`**：`pipeline::BatchNames` 批内认领——同批同 stem
    成品自动 `<stem> (2)`（大小写折叠），GUI 摘要 + CLI 开跑前披露；CLI
    在工人池启动前定名（调度不可再改名）。单张导出保持 stem 键不变。
  - **批23 `dc17f23`**：非 Accept 判词全面不自动入档——GUI 停在画布
    （● 亮、暂存/关窗守卫保护、Ctrl+S 保留）、web `saved:false`+警告且
    客户端只在 `data.saved` 时基线化（顺带修掉已保存分析后的假"未保存"
    确认）、CLI analyze/auto 门控（重定向 -o 照写；auto 仍出成品并打印
    配方）、批量记 NotAccepted（留 pending，重跑会再计费——摘要明说）。
  - **批24 `473d634`**：恢复链纳入 **RAW 旁的 Lightroom sidecar**
    （`store::lightroom_sidecar`：最新意图胜出；我们自己拷出的投影按字节
    识别并跳过；平手归 store；恢复绝不动 store 副本）。GUI kind 披露 +
    web `X-Recipe-Source`。已知边界：图库 ● 徽章/批量 pending 仍只看 store。
  - **批25 `59e9cbd`**：Save XMP 改**合并**——`merge_recipe_into_xmp`
    只替换 Autoshop 拥有的 crs 属性/子块（引号感知标签扫描 + 深度计数
    嵌套 Description——批3教训），LR 独有的 Texture/相机档案/Look/LR 镜头
    块/外来命名空间/xpacket 全保留；不可安全拼接→整份重写（旧行为）。
    边界：蒙版整块替换（LR 蒙版内部扩展字段不保留）。
  - **批26 `ba11458`**（U30 三条）：heal 引擎按位深泛型（u8/u16，16 位
    母版不再量化到 8 位）、alpha 拆出直通、母版暂存+改名发布；web 会话
    母版链（`X-Output-Path` 回传 → heal/retouch/develop/export/download/
    xmp 的 `master` 字段，必须 canonicalize 进 ./out 否则 400）——二次
    修复不再丢首次、预览以配方重渲、Save 把会话母版记入 pixels.json。
  - **批27 `c223af5`**（D10-1 根治）：**实证 rawler 在校准步就把超 sRGB
    色域钳掉**（`map_3ch_to_rgb` → `clip_euclidean_norm_avg`）→ 宽色域
    交付改为**直接在交付基色显影**：rawler 跳过 WB/Calibrate/SRgb 步，
    自做 DNG 规范校准（行归一保白），负分量存活到最终 16 位打包才钳，
    高光沿用 rawler 去饱和策略保持与 sRGB 渲染一致；工作转移曲线统一用
    sRGB TRC（中性轴与 sRGB 渲染逐值一致，测试钉死）；AdobeRGB 编码时
    仅换 TRC。sRGB 输出逐字节不变；烘焙源仍走矩阵转换。真机验收点：
    P3 导出在广色域屏上比 sRGB 导出更饱和、LR 中读数一致。
  - **批28 `947d15d`**：Codex 只读复审批22-27——8 条：4 修（HIGH 会话
    母版未绑定照片可跨照片注入 → 名字必须以 `<stem>.` 开头；HIGH api_xmp
    先提交后验证 → 母版先验后写、拒绝即 400；单引号属性剥除；宽色域高光
    分支负值预剪=与 rawler 严格一致）/ 2 接受 / 2 驳回（有据）。
  - **批29 `c88e4b4`**（用户决策①落地）：**Temp 滑杆改说绝对开尔文**——
    `render::as_shot_wb`（元数据级 dummy 解码，免 demosaic；实证 rawler
    dummy 仍带 wb_coeffs/矩阵）：cam_neutral=1/wb → inv(xyz2cam) → XYZ →
    McCamy CCT + Krystek 轨迹 Duv tint（3000/Duv 标度——D65 的 +0.0032
    正落 ACR Daylight 预设的 +10，符号与量级双钉死；D65→6504K、A→2856K
    测试锁定）。recipe 增 engine-only `as_shot_k/as_shot_tint` 盖戳字段
    （None=旧档→5500 锚→逐字节不变，测试钉死），saved-first 规则盖戳于
    produce_recipe/GUI 开图/Reset/粘贴（按目标重解析）/serve 四处/store
    派生快照。引擎锚定盖戳值（目标==as-shot=真无操作）；吸管解算带锚；
    GUI 勾选自定义 WB 即落在 as-shot（不再跳 5500）、双击复位=as-shot、
    "as shot ≈ N K" 标注；web 全镜像。XMP：盖戳照的仅-tint 编辑改写
    Custom 钉在 as-shot K（关掉"LR 无视 As Shot 下的 Tint"披露缺陷）。
  - **批30 `cdadd5e`**：Codex 复审批29——5 条：3 修 / 1 部分 / 1 接受。
    HIGH=旧代 Autoshop 自产 XMP 的开尔文被重释为绝对值（四恢复面）→
    根治：写方启用纪元标 `x:xmptk="Autoshop 2"`（合并时升级旧标），
    `xmp_to_recipe` 对旧代 Autoshop 带温度文档**钉锚 5500**（"按 5500
    调的"之诚实编码，tint 留 None=不冒充相机值、标注随之不显示），全部
    盖戳点改**仅 None 才盖**；LR 外来 sidecar 不钉（其温度本就是绝对）。
    MED=Generated 画布对烘焙像素冒充绝对 K → 锚纳入既有剥离规则（全部
    剥离点+Reset；save-quit 由 `photo_calibration` 三元组一次快照回盖）。
    LOW=锚在源头取整（XMP 整数往返正中锚点）；CCT 接受带收至 1667–15000K
    （带外元数据=垃圾→锚保持未知而非错误绝对标签）。
  - **批31 `ff047b6`**（A4 几何队列关闭）：serve `source_dims` 改读
    `develop_base` 缓存尺寸（原每次解码内嵌预览且其长宽比≠传感器 3:2，
    映射整体偏移；映射数学对 dims 齐次已核实）；web 选区框改百分比定位
    （原 CSS 像素，窗口缩放后框与实际分析目标脱钩）；径向放置 bbox 超集
    =模型无旋转椭圆的既定近似，就地披露；web 修饰后未拉直显示已由批26
    的 renderAfter 重渲修掉（双处理器核实）。
  - **批32 `03129ef`**（A6 披露余项，"成品不再撒谎"）：①不可读蒙版栅格
    →原成品"成功"却缺一笔编辑：唯一成品漏斗 `render_to_file` 拒绝并点名
    蒙版（GUI 单张/批量、CLI、web 全继承）②pixels.json 母版链断→原静默
    导出未修饰源并报成功：新 `store::render_source_checked` 成品面拒绝
    （serve 500/CLI bail/批量单张计失败），web 预览降级但带
    X-Preview-Warning；顺手关掉批30缺口（render_source 的 generated 剥离
    补剥 as-shot 锚，CLI/web 渲染与 GUI 画布一致）③XMP 数字损坏→静默
    归零且下次保存覆盖原件：新 `xmp::unparsable_crs_numbers`（与 crs_f32
    解析规则逐字节同）+GUI 开图披露+web X-Recipe-Warning+store 派生快照
    stderr。注：A6 原始 21 条枚举随上下文压缩丢失，台账所记三家族逐一
    对现码重验后根治——如实披露。
  - **批34 `9583f22`**：Codex 复审批31-32——3 条全确认全修：MED 披露在
    noop 导入上失效（仅有的编辑即损坏→恢复为空→警告随之丢弃→下次保存
    静默覆盖损坏原件）→列表改为对每个被查阅 XMP 无条件累积、随一切应答
    （含 GUI NoopOnly/Nothing 与 web 404，测试钉死）；MED 非 ASCII 文件名
    使警告头静默失效（café.ARW）→消息改静态 ASCII；LOW 停放蒙版（默认
    amount=1、滑杆全中性）丢栅格误拒导出→门改镜像引擎自身活性规则。
  - **批35 `c87b7a1`**（A7 引擎缓冲生命周期队列关闭）：枚举从台账批17
    记录恢复（审查基点 c9e277c 的 render.rs 91/103/175/440/548 +
    generative 242 + retouch 477），逐处对基点快照溯源、再对现码重验：
    **5 修 / 1 探针后驳回 / 1 已由批26根治**。修（全部逐位不变的纯生命
    周期改动）：①`render_to_image_in` 的 RawSource（整个 RAW 文件字节
    ~60-120 MB）与 u16 传感器马赛克（61MP ~120 MB）原活到函数尾、压在
    整条 ~720 MB/平面 float 链之下→解码入独立作用域、马赛克在宽色域
    路径拷出校准元数据后立即释放；②`render_baked_to_image` 的 to_rgb16
    暂存副本（~366 MB）陪跑整个显影→转录成 f32 后即释放；③导出锐化
    分支 u16 源压在 unsharp+重打包下（峰值 rgb+data+buf ≈1.46 GB）→
    转录后释放；④`blur_plane` 的 `src.to_vec()` 种子=每次清晰度/降噪/
    锐化调用多一整平面（~240 MB）拷贝、随即被首个横向 pass 替换→首
    pass 直读调用方平面；⑤生成式填充的 16 位母版（~366 MB）闲置陪跑
    分钟级模型调用、调用后峰值 母版+瓦片+权重+合成 ≈1.1 GB→调用前
    先转 8 位合成形态（等待期持 ~240 MB，峰值少一整帧 16 位）。
    **探针驳回**：cap 提前到 orientation 之前（竖构图预览可免一份
    ~720 MB 全幅旋转）——自写换序测试当场抓住错误假设：
    `image::thumbnail` 整数分箱对方向变换**不等变**（翻转/90/270 错一个
    源像素；Transpose/Transverse 在平滑梯度上偏差达 0.48，且 Rotate90
    与 Transpose 行为不一致无法解释）→不上船；探针钉为金丝雀测试
    `thumbnail_binning_is_not_orientation_equivariant`（将来 image crate
    若变等变，测试反向失败提示优化重新可用）。**已由批26根治**：
    retouch heal 原 `base.to_rgb8()`+base 双持整场编排——批26 原生位深
    改造已让 heal 按值消费 base、检测直接借用缓冲缩放。
  - **批36 `686de99`**：Codex 复审批35——2 条全确认全修（另 6 项
    confirmed-clean，含 rawler 0.7.2 `Intermediate` 自持无借用的源级
    核实）：MED 提前合成转换让**已取消**的填充先白付一次全幅转换→
    转换前先验 `cancelled()`；立即失败的调用仍先付亚秒级转换（对分钟级
    收益接受并注释）。LOW 金丝雀失败消息在 0.1 阈值下宣称"优化可用"
    过头——更小但非零的发散依然禁止换序→消息改为要求重跑八方向探针。
  - **批37 `a6712b7` + 批38 `1740387`**（D13 LR 键位/交互对标队列关闭，
    23 条）：枚举**原文**从 D13 单元的 codex 交付转录整份找回（23 条含
    file:line 与理由），六单元只读 workflow 舰队对现码重验：22 条仍在、
    1 条部分已修；一个子断言被实证驳回（egui 0.29 上"焦点在字段时 Esc
    关不掉设置窗"不复现）。GUI 半场：渐变拖拽方向翻转对齐 LR（按下=
    完全应用侧，提示/状态/文档/中文同翻）；Shift 锁轴画渐变；满幅裁剪
    框贴边处让出 16px 内侧旋转带（原承诺的框外拉直手势不可达）；比例
    预设即时应用（同心最大适配）；Enter 提交裁剪；裁剪中方向键微移
    框（0.5%/次，Shift ×10，退出后恢复图库导航）——键盘几何的裁剪半
    场，蒙版几何仍指针限定（接受入档）；裁剪中 O 循环网格线（三分→
    黄金→关）不再误翻蒙版覆盖层；\\ 锁定原图对比、Y 切换并排/单图；
    Ctrl+Shift+E（LR 键位）先于 Ctrl+E 消费（别名保留）；W/Q/K/M/
    Shift+M 五个 LR 工具键各镜像其按钮；设置窗与速查表对键盘全模态
    （Esc 前置层 + 两层门共同守护，原 R/O/J/方向键会改动窗后照片）；
    退出确认对话框加点击吞噬遮罩+置顶（原对话框背后控件仍可点）；
    速查表 22→30 行补 Ctrl/⌘ 双标与未记载的 Shift+Z 重做；Frac 滑杆
    微调下限钉在显示网格（0-0.5 羽化的 ArrowDown 原是永久无操作）。
    Web 半场：Ctrl+S 存显影（原触发浏览器存页面）；撤销历史条目扩为
    {recipe, master}——填充/修复入历史可被 Ctrl+Z 退掉；设置窗真模态
    （焦点入窗+Tab 陷阱+Esc 关闭还焦点+文档级快捷键回避，原 Ctrl+Z
    在设置单选钮上会撤销窗后照片的显影历史）；缩略图键盘按钮化（原
    纯键盘用户根本无法选照片）；After 图可聚焦+选区键盘路径（方向键
    移动/Shift 缩放/Enter 确认/Esc 清除）——蒙版绘画按自由笔划语义
    保持指针限定（接受披露）；四族生成滑杆补 aria-label+aria-valuetext；
    全族双击复位（blending→50、Temp→as-shot 锚）；Temp 轨道对数化
    （镜像 GUI SliderFeel::LogK）+ 可编辑开尔文数字框（as-shot 标注
    移入占位符，双半门保留）。
  - **批39 `717c960`**：Codex 复审批37-38——6 条：4 修 1 部分 1 接受
    （层级重构/遮罩/选区路径判 clean）：HIGH web 未保存基线只比配方
    →滑杆未动的填充点缩略图即静默丢链→基线快照改 {recipe, master}
    成对；MED 预设旗标在工具关闭时延迟触发成惊吓→只在裁剪工具开启
    时武装；MED 预设重推导可低于拖拽路径 0.05 下限→补最小值并重夹
    位置；LOW 武装态画布提示仍教旧方向→翻转+中文+Shift 提示；LOW
    部分=对数轨道 1001 档 ~0.3% 量化（配方始终存精确开尔文，仅显示
    侧，已注释披露）；LOW 接受=web 无 JS 回归测试骨架（既有取舍）。
  - **批40 `c596caf` + 批41 `9900654`**（U14 弱测试队列关闭——审查队列
    至此**清空**）：U14 枚举**原文**找回（18 条；两条 HIGH 已由批20 修）
    + U10 五条原文找回（#1=批19 退不出陷阱）；剩余 20 条经五单元只读
    workflow 舰队对现码重验：19 仍弱 / 1 驳回（位图蒙版缓存身份——
    claim_raster 每次重跑领新路径，(path,mtime,len) 结构上不可能吃旧；
    与 R4-18 先例一致）。**头条：新八态方向测试上跑即抓住真回归**——
    批35 把 `fliph()` 换成 `flip_horizontal_in_place(&mut DynamicImage)`，
    而 DynamicImage 的 GenericImage 适配器像素是 Rgba<u8>：自批35 起每张
    Transpose/Transverse 方向照片的整条 f32 工作缓冲被**量化到 8 位**
    （且被钳位——宽色域负值即死）。修复=按变体分发的类型内翻转。这是
    战役抓回的**第四个自伤回归**。修复随即反转批35金丝雀：八方向重探
    证明旧"Transpose 偏差 0.48"全是量化伪影——真相=分箱与纯轴交换
    （Normal/Transpose）**精确对易**、与六个含反转方向差一个源分箱；
    金丝雀重写为双侧（精确臂兼作翻转修复钉扎），两处散文记录同步更正。
    引擎另修：去雾气光采样对列奇偶相位锁定（gcd(stride,w)>1 时预览与
    导出失配，破坏文档承诺；≥524288 px 且 dehaze≠0 的渲染略变，如实
    披露）；两条晕影通道补零尺寸守卫（par_chunks_mut(0) panic）。13 处
    测试加固各杀一个此前存活的具名变异（线性蒙版内部行、NR 控制渲染
    逐位等值、范围蒙版亮度保持、sRGB 配置文件字节钉扎、绿/蓝基色全程
    探针——列换变异连批27交叉校验也躲过、宽色域 PNG 经解码器比对+假
    注释更正、blending=100 恒等断言、晕影线性光公式、镜头斜坡+CA、
    方向八态表、导出锐化边缘能量、锐化平场对照、HSL 绿通道+亮度推灰）。
    GUI 半场真修：待提交蒙版改名原只有 Ctrl+S 冲刷——Save 按钮存旧名、
    缩略图/Ctrl+O 暂存旧名、关闭守卫按未冲刷配方判净直接退出丢改名→
    冲刷移入三入口（save_xmp 头部/open_path 暂存前/关闭守卫脏检前）+
    回归测试；geometric_view_mapping_roundtrips 补离心非惰性钉扎（全
    套件唯一覆盖 lens_geom_norm 档案分支者）；"curve_editor"测试从未
    进过 curve_editor→按实改名 + 新驱动测试无头跑真编辑器（egui
    Context::run + 合成指针事件三趟，#[cfg(test)] 矩形缝）断言点击加
    点落在**选中**通道；异步显影测试补真派发（armed/消费/二次吞掉），
    完成断言不再空洞。
  - **批42 `0cff46f`**：Codex 复审批40-41——1 中 3 低（方向表/egui 驱动/
    异步/各容差判 clean）：MED 互素上跳可塌采样预算（宽 15015 → 步进
    17，~1/8）→根治=**剪切网格**（逐行相位 +1，构造上免疫 gcd、预算
    精确、gcd 助手删除）；LOW CA 断言对称形式放过 R/B 互换校正→收紧
    为严格方向（实证过）；LOW 改名测试未驱动 save_xmp→借 generated
    变体早退无盘驱动（头部冲刷先行），关闭守卫不可无头驱动已在测试内
    如实披露；LOW 接受=WB 吸管/范围拾取 5×5 采样经 8 位适配器——用户
    瞄准的画布本就是 8 位显示纹理，拾取精度与意图面一致，留档不改。
  - 终态：批35-36 后 **a405557..5354821 已按用户"推送"令上线**；其上
    批37-42 共 **7 提交未推送**；171 lib + 1 cli + 12 gui 双配置全绿；
    clippy 双零；i18n 445/473/0/0。**R11 裁决队列全清**（余 U14 弱测试
    已关闭）。真机验收点
    （WB 批）：开 RAW 见"as shot ≈ 相机实拍 K"、勾自定义 WB 画面不跳、
    Temp 数值与 LR 同片读数同义、旧存档观感逐字节不变、旧 XMP-only
    存档渲染不变；（A4/A6 批）：窗口缩放后 web 选区框不漂、断母版链
    导出报错而非假成功、损坏 XMP 打开即见披露；（A7 批）：61MP 竖构图
    预览/导出/生成式填充峰值内存显著下降、渲染输出逐位不变；（D13 批）：
    渐变从按下侧全强度、Shift 锁轴、满幅裁剪贴边可旋转、比例预设即时
    生效、Enter 提交裁剪、W/Q/K/M/\\/Y 全响应、设置窗开着时快捷键不再
    误伤照片、web Ctrl+S 存显影、填充后 Ctrl+Z 可退、纯键盘可选照片
    与选区、Temp 轨道 4000-8000K 段有真行程；（U14 批）：横平竖直翻转
    相机（Transpose/Transverse 方向）的照片渲染不再有 8 位阶梯、蒙版
    改名后点缩略图/Save/关窗均不丢新名、大幅面照片开去雾预览与导出
    观感一致。
- **R11 三轴 64 路全面审查（2026-08-04，用户令"再跑一轮64路并行 gpt 5.6
  sol 最高推理等级审查，彻底排查代码/架构/设计缺陷"）**——收敛战役之后的
  再审，按用户三个词各成一轴切片：**代码 34 单元**（gui.rs 10 片 / render.rs
  4 片 / serve、web、fit 各 2 片 / 其余模块与 python 边车逐一）+ **架构 16
  单元**（持久化原子性、recipe 单一真源、并发模型、校准语义、几何映射栈、
  错误披露、内存与缓存、安全面、AI 集成、未保存不变量、XMP 互操作、测试
  覆盖盲区、GUI 状态机、配置分层、web 代际、模块边界）+ **设计 14 单元**
  （色调/WB/HSL 与分级/锐化降噪/镜头/裁剪/蒙版/AI 流程/修饰/导出/版本/
  图库/键盘/web 对等）。63/64 单元返回约 **372 条 findings**，逐条对码
  裁决后落地 **20 个修复批 / 21 提交**（全部未推送）。要点：
  - **数据安全**：崩溃后只剩 `.bak` 的存档无人读取会被下次保存覆盖 →
    `store::recover_orphan_baks` 在每次触碰时修复；备份门跟随"读者会恢复
    什么"（中性配方 + 已编辑 XMP 走 XMP 备份路径）；只读图库守卫原本只护
    RAW 自身目录，兄弟目录已存在照片可被 `-o` 覆盖 → 免配置的"输出区之外
    绝不覆盖已存在图像"规则；载入版本后删该版本会删掉画布正在用的栅格 →
    `store::detach_rasters` 载入即认领私有副本；成品导出改暂存+原子发布。
  - **安全**：本机 web 服务器无 Host/Origin 校验（DNS rebinding 可读设置
    含 API key）→ 回环校验 + 同源要求；照片 id 无文件夹代际绑定 → 代际戳
    + `X-Autoshop-Gen` + 409；降噪边车从**可变上游分支**下载 py 模块并
    `exec_module`（RCE）→ 钉不可变提交 + SHA-256 校验（含旧缓存）+
    `weights_only=True`；claude 验证器子进程带内置工具运行而提示词含模型
    撰写文本 → `--tools ""`（帮助文本 + 实跑双验证）。
  - **AI 破坏性**：严格响应 schema 无法表达位图蒙版/调色增益/手动镜头
    三字段，而 Refine 结果自动保存 → **点一次 AI Refine 会删掉 AI 选区/
    画笔/反推分区蒙版**；`pipeline::carry_over_unrepresentable` 从基线回挂。
  - **计费**：图像生成传输失败 30s 内自动重发（非幂等、按次计费）→ 窗口
    收至 3s；分析路径 JSON **解析**失败被当传输失败重发（端点已应答并
    计费）→ 只重试真正的读取失败。
  - **引擎/成像**：色彩分级 `blending` 被实现成振幅乘子而非区域重叠（0 =
    静默关闭全部区域轮，默认 50 = 全体调色照片只出一半强度且与 LR 不一致）；
    曲线重复控制点造成单格断崖；位图蒙版取样与产生方坐标约定不一致（同尺寸
    蒙版采不到最后一行列，且预览与导出落点不同）。
  - **状态一致性**：解码基图缓存键不含母版身份 → 反推后切走再切回复活旧
    母版（三单元互证）；后台变体的未保存编辑既不进暂存也不触发关窗守卫 →
    守卫拦截并如实告知"只有活动变体能保存"；web 端**完全没有**未保存基线
    概念 → 切换/刷新前确认。
  - **自伤回归两次，均由"修复提交也进下一轮审查"抓回**：R8 的注释感知
    XMP 扫描对 BOM 会 panic（R11 批 3 修）；R11 批 11 的变体守卫使
    「放弃并退出」陷入死循环、应用退不出去（批 19 修）。
  - **弱测试点名并加固**：原子发布测试只观察成功导出（非原子实现同样通过）
    → 加"失败导出不得动旧文件"；坐标映射往返测试只证两图互逆（双双退化成
    恒等也通过）→ 加"非零几何必须移动偏心点"。
  - 终态基线 **152 lib + 1 cli + 9 gui** 双配置全绿，clippy 双零，i18n
    432/460/0/0，JS parse OK。**未推送、未发布**。
  - **待用户拍板**（已裁决为真但需产品决策，未擅改）：WB 非相机原生模型
    （Temp 不是绝对开尔文、XMP 温度对 LR 不诚实）；导出前钳到 sRGB 使宽
    色域导出名不副实；成品文件名只用 stem 导致跨目录同名互相覆盖（**四个
    单元独立报告**）；非 Accept 判词是否允许自动保存。
  - **已裁决待成批**：A11 恢复路径从不读 RAW 旁边的 LR sidecar（LR 里的
    修改会被更旧存档盖过）+ Save XMP 整份重写丢 LR 独有属性；U30 heal/clone
    强制 RGB8（16 位与透明度丢失）、web 二次修复丢首次；A4 几何映射五条；
    A6 披露 21 条；A7 引擎缓冲生命周期七处；D13 LR 键位差异 23 条。

- **收敛战役完成（2026-08-03/04，"多跑两遍，跑到收敛"的完整执行）**——
  第三轮之后又跑了七轮（R4-R10，全部订阅 codex gpt-5.6-sol xhigh 单元 +
  主代理逐条对码裁决），findings 轨迹 **190→88→26→8→11→3→1→0**，第十轮
  空数组宣告收敛。累计 **7 提交约 295 项核实修复，全部未推送**：
  `b4c6c30`(R3) `0deca42`(R4) `545b50f`(R8) `542b1cf`(R9) 与
  `7768344`(R5) `af56b14`(R6) `4b13fc6`(R7)。R4-R6 要点见台账
  （adjudication64.md）；R7-R10 增量要点：
  - **R7 关键实证**：`fs::rename` 在 Windows 上**会**替换已存在目标
    （探针钉死；库内 5 处 "cannot replace" 注释全为假信念）——因此
    create_new 认领 + rename 发布留有覆盖窗。根治 = `publish_no_clobber`
    （同目录 pid+seq 暂存 + `hard_link` 原子不覆盖发布；exFAT 回退
    认领+rename 且清理仅在 len==0 时）。迁移栅格回滚删除取消（并发
    采纳竞态）；`|| to.exists()` 不再把自家 0 字节认领残留当已迁移；
    迁移门 metadata 三分诊（不可访问=可重试而非"无遗留"）；same_path
    Windows 大小写折叠（缺失叶 -o 不再绕备份门）。**同轮抓回 R5 自伤
    严重回归**：config 设置保存双 create_new 同一 tmp → 自 7768344 起
    每次保存必败——已并单开持句柄根治（该函数无测试覆盖，已记）。
    另修 web 翻页越代、serve 三处显示源尺寸、区域 9 点/边采样、xmp
    注释感知出处扫描。
  - **R8**：全部六个 tmp 铸名点（pixels/备份配方/备份 XMP/copy_atomic/
    sibling_tmp/write_recipe）改共用**一个**进程级 `next_tmp_seq()`——
    独立计数器各自从 0 起步、同词法空间撞名可让版本备份与迁移发布互相
    截断暂存字节、备份门发布错误快照（HIGH）。web setdir 失败双分支
    seq 守卫重渲旧文件夹（不再搁浅）；xmp `<x:xmpmeta` 后强制名字边界
    （`<x:xmpmetadata>` 前缀伪造失效）。
  - **R9**：!r.ok 分支 body 读取纳入 try（mid-body 断连不再绕过恢复）。
  - **新录接受取舍**：exFAT 回退微秒残窗；NTFS 逐目录大小写敏感目录内
    same_path 大小写折叠会合并真不同叶（默认不敏感才是绕门主案）；
    409 后重渲预换表文件夹（多窗口固有，代际 id 守卫兜底）；迁移暂存
    栅格不回滚（孤儿归被取代栅格边界）。
  - 终态基线 **142 lib + 1 cli + 9 gui** 双配置全绿，clippy 双零，
    i18n 431 调用点/459 条目/0 未译/0 漂移，JS parse OK。**未发布**
    （下次发版=minor bump，含本战役全部 7 提交）。

- **第三轮 64 路 gpt-5.6-sol 最高推理舰队全量复审 + 修复批（2026-08-03，
  用户令"派发64路并行codex gpt 5.6 sol最高推理等级进行全面代码审查…
  多跑两遍，跑到收敛"；中途按用户令全面切换到订阅 codex 通道）**——
  64/64 单元产出（56 存量 + 8 订阅补跑 + 1 重发），190 findings 三轮
  分批裁决：**~120 确认全修 / 10 驳回有据 / 9 记录立项**，跨 24 文件。
  要点（细节见会话台账 adjudication64.md Batch 1-7）：
  - **数据安全**：CLI Analyze/Auto/Match 三命令补备份门（HIGH，三端契约
    补齐）；迁移 cwd 旁观者文件保护（HIGH）；store 原子拷贝/vMAX 拒绝/
    崩溃残留冻结栅格重置/死引用永久惰性/pixels.json 三竞态；write_recipe
    陈旧 .bak 预清 + 目录拒写；guard_readonly 别名根治 + 导出对渲染源与
    原 RAW 双重把关；分区反推栅格改**每次 fit 唯一认领名**
    （store::claim_raster，GUI+CLI 共用——写在配方落盘前的就地重写窗口
    根除）；fit 计算成果在写库失败时降级为画布未保存而非丢弃；持久化
    fit 同步 clear_pixel_source（配对语义）；设置文件 tmp+rename +
    unix 0600。
  - **校准语义钉死（InPlace=中性显影，Generated=像素含观感）**：Analyze/
    Reset/版本载入/批量导出四路径改只对 Generated 剥 base_curve+
    lens_profile（原 base.is_some() 误伤 InPlace 全部纠正）；Before 四处
    改按画布配方曲线渲染（InPlace 不再假暗 0.6-1.4EV）；Generated 恢复
    时 saved_recipe 同步归一化（假 ● 根除）；两处 v0.14 前"烘焙像素自带
    相机观感"陈旧注释改真相。
  - **GUI 交互**：同路径重开保留未保存工作（HIGH）；Save-all 活画布无条件
    压制陈旧 stash + busy 窗口竞态修；快捷键两级门（Ctrl+O/E/S 焦点下
    可用）；退出层 Enter 默认真可用（布防即交焦点）；WB 滴管逐像素采样；
    HiDPI 1:1/百分比修；radial 边把手增量拖拽；放置四角映射+最小尺寸；
    画笔局部尺度换算；定比角部主导轴按拖拽增量；蒙版拖拽越界守卫
    （panic）；亮度端点 clamp 防振荡；CA 缺通道按通道补层；重试/清扫：
    五个修饰 worker 失败释放 0 字节认领名；busy 期重绘 100ms 节流。
  - **web**：换目录全代际失效+清网格；选照清 debounce 竞态；fill∥heal
    互斥；r.ok 先行真相化 stale 消息；pageOffset 成功才提交；画布重设
    保留已画蒙版；设置窗加载门+代际；saveXmp 防重叠；700px 响应式断点。
  - **解码/元数据**：解码上限有界化（65536²/4GiB，替代 no_limits）；
    烘焙图应用 EXIF 方向（导入 JPEG 不再侧躺）；Meta 尺寸改显示帧
    （竖拍风格特征修正）；APEX/有理数非有限过滤；直方图只降采样。
  - **XMP/eval/style**：feather 文本形状消歧（自家 1% 往返不再变 100%）；
    CropAngle 按 HasCrop 门控；元组严格解析防移位；出处检测改属性形匹配；
    as-shot WB 溯源规则进 eval+style（相机 Kelvin 不再算用户编辑/风格
    偏好）；style 温度混合不再强改 as-shot；索引 tmp pid 唯一；不可读
    sidecar 跳过样本。retouch：full_res 对烘焙源生效；空修复计划拒绝
    写零斑点母版。denoise/segment：原子发布蒙版、.part/tmp 清扫、NaN
    强度防线。图库导入改单次目录扫描（migrate_legacy_from_many，
    O(photos×entries) 根除）。i18n 文案真相三条。
  - **驳回有据（10）**：paste 继承源曲线/范围蒙版参考图无几何/覆盖层
    display-only 降采样/Tab=LR 键盘语法/overlay_ref 全交换点清点/busy
    双门挡陈旧 Fitted/produce_recipe 无 saved 早退/omit 独立成列/web
    Reset 已回填校准/provider 翻转有 same_base 自失效。
  - **记录立项（9）**：load_active/overlay 结算帧同步显影异步化；12× 缩放
    帽 vs 真 1:1；全幅解码转预览内存（并入结构性内存池）；fit 深层 DSP
    四项（饱和度次序非对易/异色净额否决/中性总体身份/岭回归活跃集）；
    分区栅格孤儿清扫。
  基线 **137 lib + 9 gui** 双配置全绿，clippy --all-targets 双零，
  i18n 432 调用点/461 条目 0 未译 0 孤儿。**未发布**（下次发版=minor bump）。

- **v0.15.0 RELEASED（2026-08-03）**——内容 = 自 v0.14.3 后全部四提交：
  在案后续工作全清批（a8f67d2：pixels.json 像素持久化 / worker 取消+进度 /
  61MP 内存八子项 / web 区域几何逆映射 / 字体·模型指纹·临时清扫）、用户
  config dotenv_override（8e9ae1f）、第二轮 32 路舰队 30 项修复 + 死代码
  五角度清查（b2591e5）。发版细节（tag、资产字节）见 git tag v0.15.0 与
  GitHub Release 页。

- **第二轮 32 路 gpt-5.6-sol 舰队复审 + 修复批 & 死代码清查（2026-08-03，
  用户令"再开32路…确保项目实现完美"+"清理一轮死代码（多次确认）"）**——
  fleet32b（契约头补全清批新设计）32/32 成功，115 findings
  （13H/45M/25L/28UX/4info，43.5万 in/8.7万 out tokens），逐条三裁：
  - **HIGH 6 修**：批量导出改从 pixels.json 母版渲染（配方叠其上——批量
    导不再丢修饰）；Ctrl+S 次序重排（像素身份先于徽章/基线/暂存推进，
    pixels 写失败保留 stash）；恢复路径 Generated 剥 base_curve/lens_profile
    （生成像素自带观感，防双重烹调）+ 两处复活的"母版自带相机观感"撒谎
    注释改中性显影真相；Analyze 落库同步写/清 pixels.json（+镜像字段）；
    `unique_out` 改 create_new 原子认领（取消后重跑不再同名互写）+
    off-by-one（tag-1000）修 + GUI 临时蒙版名加原子计数（pid 撞名）；
    main.rs process_one 改 render→XMP→recipe（完成标记真正最后落——
    render 失败不再被 resume 永久跳过）。
  - **MEDIUM/LOW 实修**：guard_readonly 根部 `..` 折叠修（"D:/../图库"
    弹掉 RootDir 得盘相对路径绕过守卫——+测试）；recipe.clamp 亮度范围
    NaN 界会 panic f32::clamp 断言→非有限整体丢弃（+测试）；Chat SSE
    finish_reason≠stop 截断显式报错；find_sources 符号链接环 64 层深度
    帽；下载渲染失败即时清 tmp；write_pixel_source 终发失败还原 .bak +
    clear_pixel_source 改 Result 三端处理 + backup 版本号 saturating；
    heal 检测缩图免整幅克隆（61MP 全幅 heal 省 ~183MB 瞬态）；heal 重叠
    斑点 last-wins 注释改真话；xml_escape 补 U+FFFE/FFFF；XMP tint-only
    As-Shot 有损边注释在案；apply_masks 全恒等调整跳过整幅扫描；
    apply_lens_distortion 零尺寸守卫；downscale_f32 0 边夹取（自伤修）；
    模型列表供应商切回 API 自动还原 stock URL。
  - **UX 实修**：●未保存涵盖像素身份（pixels_on_disk 镜像，逐帧零磁盘
    IO）+悬停文案；Tint 滑杆随 Custom WB 门控；四处 Full-res 勾选 RAW-
    only 门控；GUI region 框四角映射（旋转下两对角框错/近退化）；定比角
    把手横向余量夹取；文案真相批（设置保存说明/两处忙碌文案改"✕ 取消
    可停"/Save develop 悬停含母版链接，i18n 双侧 422 键 0/0）。
  - **web 实修**：fill/heal 结果带 (id, selectSeq) 双守卫（A→B→A 同 id
    也不串台）；analyze 守卫补 seq；unhandledrejection 全局兜底（传输失
    败不再永挂"…ing"状态）；fill 提示 OPENAI_API_KEY 必需说法改真相；
    风格索引 build 成功后按钮复活。
  - **驳回/在案**（证据）：NoopOnly 弃校准=d1f6986 设计；fit 不走
    produce_recipe=在案设计；heal/clone 中性基图=b1c f7ce 契约；迁移
    TOCTOU/非UTF8键/中断残留=v0.13 取舍；CLI heal 固定名=交付物语义；
    404|422 协商=v0.14.3 裁定（含结构化归因门）；600s 下限 env 旋钮语义；
    claude 验证器 output() 双管并读无死锁（std 语义）；crop_imm 截断；
    批量 3 线程池=v0.11 设计；茶隼级微项（trf 占位符注入/同 mtime 缓存
    陈旧）record。**立项（UX/内存遗留）**：export_mask_png UI 线程编码
    （8192 卡顿）；几何中笔划降采样重映射；窄面板换行批（曲线 160px/
    蒙版工具条/反推行/状态栏●溢出）；AI region 覆盖框几何映射显示；
    Before 缺镜头校正对比；px 重解码烘焙变体文案；flex_size 独立取整可
    破 3:1；segment.py float32 全幅蒙版；denoise sidecar 不可中断+
    torch.hub 代码未钉版本（供应链注记）。
  - **死代码清查（用户令，五角度多重确认）**：①编译器 warning-clean=
    私有/pub(crate) 零死代码 ②163 个 lib pub 项全库交叉引用零未引用
    ③Cargo 18/18 依赖在用 ④web 46 个 JS 定义零死 + i18n 450 条目零孤儿
    （28 条经常量表间接存活逐一核实）⑤python 11 个 def 零死。结论=**无
    可删项**（本会话唯一转死的 composite_region 已即时 cfg(test) 处理）。
  基线 137 lib + 9 gui（NaN 范围/根部 `..` 两用例并入既有测试），clippy
  双零，i18n 422 调用点 450 条目 0/0，web JS node --check 通过。

- **在案后续工作全清批（2026-08-03，用户令"把所有后续工作推完"）**——
  此前三个 backlog 池（v0.14.1 内存专项 4 项 + v0.14.2 待办 2 项 +
  32 路舰队立项 7 项）一次性全部落地：
  - **像素修饰变体关联持久化**：新 store 边车 `pixels.json`
    （store.rs `write/read/clear_pixel_source`，库外 origin 绝对化、库内
    裸名）；Ctrl+S 随 recipe 写/清（gui `save_xmp`，保存文案改真话——
    重新打开会恢复）；打开 worker 连带解码烘焙母版（`OpenedBase` 第 4 元
    `BakedBase`），fresh-open/px 重解码两分支恢复（px 重解码还把 undo 栈
    Arc 重指新分辨率母版）；`nav_stash` 升级为 `StashEntry`（recipe+像素
    身份，wholesale 优先于磁盘）；退出层 pending 携带像素身份、Save-all
    同写；关窗守卫把"像素未保存"也算 unsaved；`forget_open_base` 在修饰
    落地/保存/清除时废弃 LRU。验收：修瑕疵→保存→关→重开，画布仍是修
    过的像素。
  - **GUI worker 取消 + 进度**：状态栏 busy 时对修饰族显示「✕ Cancel」；
    取消=epoch 弃用（`gen_epoch`/`Msg::Retouched(u64,..)` 迟到结果整体丢
    弃、UI 立即解锁）+ generative 线程局部 `WorkerHooks`（fill/reimagine
    传播 cancel 旗标：协商环顶/每 SSE 事件/合成前三处检查点真停流）；
    partial-image 心跳与协商备注经 `Msg::Progress` 进状态栏。
  - **61MP 内存专项（8 子项全落地）**：① render_to_image 加 `max_edge`
    （orient 后 develop 前 f32 降采样 `downscale_f32`，demosaic 后全链在
    工作分辨率跑）——9 调用点接线：GUI 开照 Some(edge)（开照不再全幅驻
    留）、heal/clone/fill/denoise 预览基 Some(2048)、reimagine 高清输入
    Some(2×目标边)、photo_base_knots Some(2048)、web develop_base
    Some(PREVIEW_EDGE)、导出 None；② 生成合成缓冲生命周期分级（mask_full
    即建即弃成 weight、16-bit base 提前 drop、box_blur 复用入参、
    composite_in_place 原位混合）——61MP 全幅填充峰值 ~1.8GB→~750MB，
    输出逐位不变（composite_region 留作 cfg(test) 包装）；③ 色彩取样点击
    改共享覆盖层 `overlay_ref` 缓存（pre 构造与 overlay 对齐：几何字段剥
    除后键相等；miss 时反向预热 overlay），采样 25 像素 get_pixel 替代整
    幅 to_rgb8——2560/4096 预览下点击不再 100-300ms 冻结；④ plan_from_mask
    painted/seen 改 u64 位集（1/8 内存）+ 坐标表改 u32 索引（减半），半
    径数学逐位不变；⑤ fit_zoned 分割输入 ≤2048 帽（CLI 全幅帧不再写
    ~180MB PNG 往返 python；蒙版为归一化数据，引擎任意分辨率重采样）；
    ⑥ CA 逐通道单采样 `sample_bilinear_ch`（逐位同 math，省 2/3 插值
    功）；⑦ 位图蒙版缓存加 256MB 字节预算（原 16 条目可钉 ~1GB）；
    ⑧ 蒙版纹理：笔刷期间脏矩形 `set_partial` 增量上传（无几何常见路径，
    原每帧全幅克隆+重传；8192 预览 ~270MB/帧）+ 有几何时中笔划 120ms 节
    流重建。
  - **web 区域选择几何逆映射**：GUI 的 view→original 复合映射下沉
    render.rs 唯一实现（`view_to_original_norm`/`original_to_view_norm`，
    GUI 变薄封装+往返测试）；analyze 带 region 时 web 端随发 `view`（取景
    配方），serve `region_to_original` 逐角 un-crop→un-rotate→正向畸变映
    射回原始帧再折入提示词（源尺寸不可读时降级为原行为）。
  - **杂项**：CJK 字体候选表 cfg 分平台（macOS PingFang/Hiragino、Linux
    Noto 各发行版布局+文泉驿）；设置窗模型列表按抓取源指纹自动失效
    （`models_from_base`，换 Base/Bridge URL 或供应商即清列表回退保底
    项，fetch 起手即清防失败残留）；serve 启动清扫 >1h 的
    `autoshop_dl_*`/`autoshop_mask_*` 残留（实证 Win11+std 打开句柄下
    unlink 本已成功——泄漏面是 AV 短暂独占后的永久残留，Windows 无自动
    %TEMP% 清理器；api_download 撒谎注释同步改真）。
  **Codex 只读复审 12 条**：8 修——#1 noop 配方+烘焙像素被"清除编辑"路
    径吞掉（Critical，真：clear 门补 `origin.is_none()`——修瑕疵后
    Ctrl+S 现在走保存路径写 recipe+pixels）；#2 Some→None 像素转变不算未
    保存（三处判定改双向比较 `read != origin`）；#3+#4 固定名母版被复跑
    覆写 / 取消后立即重跑同路径写竞态（`unique_out` 探测唯一名统一五个
    starter，含 reimagine 收编；999 上限拒绝）；#5 web 中性保存清单补
    pixels.json；#7b Progress 无 epoch（改 `Progress(u64,..)` 门控）；
    #8 pixels.json tmp 名加计数+旧档退位 .bak；#9 边车在而不可用时
    stderr 警告（不再无声回退）；#12 plan_from_mask >u32 索引空间显式拒
    绝。4 条有据取舍：#6 版本快照不含像素身份（v<N> 先于 pixels.json 设
    计，**立项**：vN.pixels 快照+恢复流）；#7a 取消对静默流最迟等到
    600s 静默上限（钩子悬停文案已如实）；#10 库外母版绝对路径不可迁移
    （./out 本为 cwd 产物，复制进库=每保存全幅 PNG 拷贝，代价不值，注释
    在案）；#11 降采样先于显影 ≠ 先显影后缩（有意语义——与 GUI 预览同
    序，CDF 统计与归一化坐标不受影响，注释在案）。
  基线 **137 lib + 9 gui**（+1：view/original 映射往返），clippy
  --all-targets（含/不含 gui）-D warnings 双零，i18n 421 调用点 0 dup/
  0 漂移（+6 键：取消/母版关联/999 上限文案）。已知边界：Generated 变体
  仍不可 Ctrl+S（无参数化配方，Save-all 例外可存）；取消对本地长算
  （python 去噪）为弃用式（子进程不杀）；web/CLI 导出与 web 预览不读
  pixels.json（GUI 级恢复，跨面统一另行立项——web 清除侧已对齐）；版本
  快照不含像素身份（立项见上）。

- **32 路 gpt-5.6-sol 双视角舰队审查 + 修复/打磨批（2026-08-03，已提交
  未推送）**——用户令"整体 GUI 再打磨一轮 + 派 32 路并行检查 debug，我统筹"。
  执行：fleet32.py（10 片 gui + 2 片 web 带 UX 授权，20 单元全库 debug）
  32/32 成功，118 findings（11H/51M/23L/26UX/7info），40.8万 in/9.0万 out
  tokens。逐条对抗核实后落地（要点）：
  - **advisor**：协商状态窄化到 400|404|422（我自己的注释/代码矛盾——401
    也在旧区间里）；SSE 中途读失败（"read AI stream:" 前缀）纳入 <30s 瞬断
    重试（error/response.failed 事件不重试——不重计费真实失败）。
  - **generative**：fidelity/size 协商同样加状态码门（原任意状态可重发）。
  - **render**：amount=0 蒙版整帧空转跳过；inscribed_dims 45° 正方形
    cos2≈0 → NaN → 1×1 修（走半对角支）；apply_lens_geometry 零尺寸守卫。
  - **recipe**：clamp() 补裁剪校验（NaN 丢弃/夹取/排序/去退化 + 测试）。
  - **xmp**：拉直-only 写 HasCrop=True+全幅载体（LR 只在裁剪态下应用
    CropAngle），读方把全幅载体坍缩回 None（往返测试钉死）；xml_escape 先
    剥 XML 1.0 禁用控制字符。
  - **serve**：上传 create_new 原子认领（并发同名不再互相截断）+失败清残
    体；setdir 双锁同持替换（raws/dir 不再错配）；/api/recipe 拒非对象
    JSON（null/数组=损坏而非新 schema）。
  - **store**：migrate 优先迁移根下的栅格（cwd 同名旁观者文件不再被 staged
    删除——数据丢失级）；delete_version 枚举失败即中止（非 NotFound）。
  - **pipeline**：guard_readonly 词法折叠 `..`（"out/../图库" 绕过修）；
    write_recipe tmp 名带 pid+原子计数（web 线程同进程并发）；.bak 还原失
    败写进报错。
  - **main**：批处理 XMP 先写、recipe 后写（完成标记必须最后落）；auto
    输出位深按扩展名如实播报。
  - **retouch**：AI 斑点上限 30 强制截断。config：NaN 风格强度过滤。
    denoise：临时文件全错误路径清理。lensmeta：CA 至少 8 节点成对。
    denoise.py：overlap 随小图收缩（广播崩溃修）。
  - **gui 状态/手感**：分析落库后 ● 基线改画布投影（烘焙变体不再秒亮
    ●）；AI-select 去重双侧剥 stem 前缀；范围蒙版切离 Color 解除取色武
    装；覆盖层失效改"Amount 前任何改动"+键去重；径向中心拖动夹取位移
    （边界不再压扁）；定比角把手取主导轴（纵向拖不再无效）；1:1 两处
    夹到渲染上限 12；As-shot 取消勾选连 Tint 一起归零；busy 期间 ✕ 拦截
    +提示；方向键无选中时从第 0 张进入；Reimagine 999 上限拒绝而非复用。
  - **文案真相**（i18n 双侧）：修饰基图注释改中性显影真相；"streams
    progress" 状态改"窗口保持忙碌"；OPENAI_API_KEY 必需说法补 OAuth 桥；
    Ctrl/⌘+click；曲线帮助不再承诺 XMP 逐位一致。
  - **web**：分页 latest-wins epoch；换目录清撤销栈；selectPhoto 作废全部
    在飞预览（A→B→A）；下载文件名快照；上传直接流 File（省一倍内存）；
    renderAfter/import 补 catch；布局改 flex（换行表头不再把应用挤出视
    口）；设置说明如实（也作用于生成填充）。
  - **驳回**（证据在案）：heal InPlace"二次显影"（基图=中性显影，注释已
    改）；覆盖层"几何双重应用"（develop_preview 无几何段）；粘贴继承源曲
    线（1488 行明文设计）；crop_imm panic（image crate 会截断）；fit 不盖
    戳/retouch 中性基图/store 迁移并发/feather=1（历史裁决在案）；env 覆盖
    低于 600s 下限（旋钮语义如此）。
  - ~~**立项/待办**：像素修饰的变体关联持久化；web 区域选择的几何逆映
    射；Windows 下载临时文件延迟清理；61MP 内存专项扩充；蒙版纹理每帧重
    传；设置窗模型列表/URL 陈旧性；非 Windows CJK 字体。~~ → **全部已随
    2026-08-03「在案后续工作全清批」落地**（见上方首条）。
  - Codex 复审 3 条全修：同进程 tmp 撞名（pid+计数）；上传失败清已认领
    残体；NaN 裁剪穿透 clamp（+测试）。
  基线 136 lib（+2：clamp 裁剪、拉直-only XMP 往返）+ 9 gui，clippy
  --all-targets 0，i18n 416 调用点 0/0。

- **真机反馈批：瞬断重试 + 主动 AI 去噪 + 提示词配按钮 + 双动词分析
  （2026-08-03，60f8444，已提交未推送）**——四项真机反馈一批落地：
  ① 传输瞬断误报根治：真机报"十几秒就弹 600s 无流活动"→ CLI 无窗复现
    **成功**（52.4s 出真配方）证明是连接期瞬断；修复 = <30s 传输失败自动
    重试一次（post_ai_json 与 images/edits 两处），报错文案改用**实测
    耗时**（远早于静默预算 = 连接/握手故障，措辞不再谎报静默超时）；
  ② AI 去噪改主动画布操作：Detail 节「🤖 立即 AI 去噪」对当前变体像素跑
    SCUNet（RAW→中性显影，默认 ≤2048 工作副本，Full-res 可选全画幅），
    走 heal 的 InPlace 基图烘焙管线（可撤销、滑杆继续其上生效）；新 lib
    入口 denoise::denoise_active；导出时去噪开关保留；
  ③ 提示词入口配专属按钮（用户："AI分析按钮得放到输入提示词那里"）：
    AI Analyze 从工具栏移入 AI 区 Direction 正下方；Reimagine 获得自己的
    提示词输入框+同排生成按钮（原偷用顶部 Direction），提取风格现在回填
    Reimagine 提示词；Generative Fill 本就合规；
  ④ 「微调」勾选框废除（预先勾选的隐藏模式）→ 双动词按钮「AI Analyze /
    AI Refine」（Refine 在编辑中性时禁用——中性时微调即分析）；
    start_analyze(refine: bool) 显式传意图，模式字段删除。
  基线 134 lib + 9 gui，clippy --all-targets 0，i18n 414 调用点 0/0。
  同日随后：32 路 gpt-5.6-sol 双视角舰队（10 片 gui+2 片 web 带 UX 打磨
  授权，其余 20 单元全库 debug）审查本提交树——结果与修复批见下一条记录。

- **v0.14.3 RELEASED（2026-08-03）**——内容 = 全 AI 调用流式化批
  （5ecbbdb，下条）。发版细节见 git tag v0.14.3 与 GitHub Release 页。

- **全 AI 调用流式化 + rationale 失实修复（2026-08-03，v0.14.3）**——
  真机第三次撞总死线：/responses 提案（推理级视觉模型）跑穿 360s →
  回退启发式基线 → 真机验证器判 Revise 并点名"rationale 与数字不符
  （说 +0.0EV/0/0，配方是 -0.06EV/-20/+4）"。两个根因两治：
  - **超时类**（v0.14.2 images/edits 方案泛化到全部文本 AI 调用）：
    advisor::post_ai_json 统一入口——注入 stream:true 走
    post_with_stall_timeout（预算=静默上限，下限 STREAM_STALL_FLOOR=600s，
    因推理期可合法静默）；Responses 族对未设 reasoning 的请求加
    `reasoning:{summary:"auto"}`（SDK 实证参数+
    response.reasoning_summary_text.delta 事件）——推理期间有真心跳；
    协商降级各至多一次（400..=422 且 error.param 实指才降：先摘要后
    stream，拒 stream 回退阻塞+原总死线）；Content-Type 分派容忍
    收下 stream 却回 JSON 的桥。四个调用点收编：propose（openai.rs）、
    describe_style、api 验证器（openai_verify.rs, Chat 族按 index==0 选
    choice 拼 delta 重组阻塞形状）、detect_spots（retouch.rs）。
    共享 SSE 分帧器 for_each_sse_json 下沉 advisor（generative.rs
    read_sse_image 重基于它——全库单一分帧实现）；error_blames_param
    结构化归因（param 优先，无 param 只认带引号提及——"upstream
    error" 不再误触发降级，generative 的 stream 守卫同步加固+状态码门）。
  - **rationale 失实类**：heuristic.rs rationale 移到 clamp+temper 之后
    （temper 软压会改所引数字：-70→-60 有测试钉死）；pipeline.rs 风格
    蒸馏真改动了配方才在 rationale 追加披露行并重验（无变化=不披露
    不重验——反向失实也堵住）。真机判词三条的归因：置信 0.4/过保守
    = 启发式回退的正常面貌（流式修复后 AI 视觉应能跑完，回退变罕见）；
    数字不符 = 上述两处失实，已根治。
  - Codex 只读复审 4 条全处置：#1 静默推理间隙→摘要流+600s 下限；
    #2 "upstream"子串误伤→error_blames_param+状态码门；#3 choices
    位置≠身份→index==0 选择；#4 无变化也披露→pre_blend 比对门。
  基线 134 lib（+5：SSE chat 重组含 index 路由/Responses completed/失败
  事件三例/error_blames_param 五例/heuristic tempered rationale）+ 9 gui，
  clippy --all-targets 0。真机验收点：分析不再"Heuristic baseline
  (…timed out…)"回退（长推理提案能跑完）；若真回退，rationale 数字与
  配方一致；风格蒸馏参与时 rationale 尾部有披露行。

- **v0.14.2 RELEASED（2026-08-03）**——内容 = 修饰流式化批（e729e6b，
  下条）。发版细节见 git tag v0.14.2 与 GitHub Release 页。

- **修饰流式化——images/edits 超时根治（2026-08-03，v0.14.2）**——
  真机第二次撞 images/edits 总死线（300→600s 重校准后，gpt-image-2
  quality=high ~8MP 仍超时）。根因 = 固定总死线对服务时长无上界的同步生成
  端点本质不适配：同一把刀同时杀健康长生成与僵死连接。根治 = **活性与时长
  分离**：
  - `advisor::post_with_stall_timeout`（src/advisor/mod.rs）：只设
    timeout_read/timeout_write（默认 600s）、不设总死线——每次 socket 读
    （等响应头/等下一分片）限时，健康长流无时长上限；语义对 ureq 2.12.1
    源码实证（stream.rs:436 无总死线时响应头等待用 timeout_read；
    response.rs:364 body 读重设同值直至连接回池）。
    AUTOSHOP_HTTP_TIMEOUT_SECS 仍统一覆盖（流式下=静默上限，非总时长）。
  - `call_images_edit`（src/generative.rs）：首选 `stream=true` +
    `partial_images=3`（对 openai-python SDK 类型定义实证：参数存在、取值
    0–3；事件 image_edit.partial_image / image_edit.completed，b64_json 与
    usage 在事件**顶层**）；按响应 Content-Type 分派（text/event-stream →
    SSE 解析，否则原 JSON 路径——服务器收下 stream 却回 JSON 也不会错解）；
    400 拒 stream/partial_images 时按本文件既有协商模式回退一次阻塞
    POST + 600s 总死线（IMAGES_EDIT_TIMEOUT_SECS 降为回退路径）。
  - SSE 解析按规范分帧：多行 `data:` 以 \n 拼接、空行=事件边界、EOF 冲刷
    未终结事件、[DONE] 哨兵忽略、512 MiB 流量上限防无界单行吃内存；error
    事件即刻报错；流收尾无 completed 时报错含已收分片数。partial 分片在
    stderr 打心跳行（"partial image N received"）。
  - 400 参数归因优先结构化 `error.param`（子串匹配仅作无 param 桥的回退）
    ——避免 size 报错文案含 "streaming" 字样时错怪 stream 参数。
  - GUI 状态文案两处 "~15-40s"/"~15–60s" 改为流式+分钟级真相（i18n 双侧
    字节同步）。
  - Codex 只读复审 7 条处置：#2 SSE 分帧 / #3 内存上限 / #4 参数归因已修；
    #1 涓滴字节可无限续命 = 不活超时语义的接受面（自配 API 端点不设恶意
    威胁模型；DNS 解析不可中断为 ureq 2.12.1 既有性质，旧总死线路径同样
    如此，无回归）；#5 三旗全拒最多 4 次 POST = 单调协商成本，接受；
    #7 GUI busy 在健康长跑期间无界禁用按钮且无取消途径 → 立项待办。
  - ~~**待办（未做，用户点单再做）**：GUI 生成/修饰 worker 取消按钮；分
    片进度透出 GUI 状态栏。~~ → **已随 2026-08-03「在案后续工作全清批」
    落地**（见上方首条）。
  基线 129 lib（+5：SSE 分帧×3、双形提取、EOF 冲刷并入）+ 9 gui，clippy
  --all-targets 0，i18n 407 调用点 dup/漂移 0。真机验收点：修饰/AI 生成在
  quality=high 大图上不再 ~10 分钟超时（console 见分片心跳行）；僵死代理
  仍在 ~600s 静默后报错，且报文说明是"流静默"而非总死线。

- **v0.14.1 RELEASED（2026-08-03）**——内容 = 64 路舰队审查修复批
  （7a061f4，下条）。发版细节见 git tag v0.14.1 与 GitHub Release 页。

- **64 路 gpt-5.6-sol 舰队审查 + 修复批（2026-08-03，v0.14.1）**——
  用户令"派遣64路并行gpt 5.6 sol进行代码检查"。执行：/v1/models 实证
  gpt-5.6-sol 存在→代码库切 64 审查单元（gui 19 片/render 9 片/其余按行
  数成组，±30 行上下文+项目契约头）→8 并发扇出（in 442,720/out 117,184
  tokens）→**64/64 成功，152 findings（13H/79M/49L/11info）**→本席逐条
  对抗核实（高危 13 条全判：7 实 6 驳，驳回依据=契约/egui API/已录取舍；
  中危抽样亲读）。修复落地（本批）：
  1. **web XSS**：图库 stem 经 escapeHtml；verdict decision 白名单入
     class 属性；escapeHtml 补引号转义。
  2. **XMP 注释消毒**：rationale 的 "--"/尾 "-" 换 U+2011（含 -- 的
     rationale 曾使整个边车 XML 非法）。
  3. **APEX 光圈**：FNumber 缺失时 ApertureValue 按 2^(Av/2) 换算（原
     直接当 f 值）。
  4. **serve**：下载改文件流式响应（原 fs::read 整个 ~366MB TIFF 进内
     存）；上传 500MB 上限（原无界 read_to_end）+ 同名避让 "name (2)"
     （原静默覆盖）；StyleIndex::save 原子发布（tmp+rename）。
  5. **GUI**：范围蒙版任何改动即刷覆盖层（原只有几何控件刷）；
     OverlayKey 补 profile 畸变/CA 开关（原切开关复用旧 warp）；粘贴带
     出处（copied_from）——贴回剪贴板来源照片保留其位图蒙版（原自贴也
     被剥）+ 打开照片粘贴后 resync；拖放受 confirm_quit 门；CA 回填查
     数据对（有红无蓝时也回填）。
  6. **杂项**：write_recipe 旧档改 .bak 退位+失败回滚（原删旧后 rename
     失败即丢权威档）；claude 验证器子进程 300s 硬预算+杀死+可
     AUTOSHOP_HTTP_TIMEOUT_SECS 覆盖（原 output() 可永久挂起）；
     delete_version 先扫栅格后删配方（原栅格删除失败被吞且不可重试）。
  7. **python 边车**：segment.py 先低分辨率 softmax 选天空通道再单通道
     上采样（原 ~150 类全上采样，61MP 下 ~36GB 必 OOM）；denoise.py
     拒绝 float 输入（原静默按 8-bit 毁图）+ alpha 保留 + tile/overlap
     守卫 + .part 下载临时名带 pid（并发竞态）。
  经读码**驳回不修**（除高危 6 条外）：HSL schema minItems（OpenAI
  strict 模式不支持会 400，代码注释已录）；config 空串键（Settings UI
  契约=空即保留）；eval 锐化 ×1.5（与 xmp ×2/3 互逆，正确）；egui
  drag_delta 累加（API 为本帧增量）；undo 24GB（基图为预览分辨率）。
  ~~**记录为内存专项待办（未做）**：非全分辨率修饰先全幅显影再缩；生成
  合成全幅多缓冲分块化；位图蒙版缓存改字节预算；开照全幅驻留优化。~~
  → **已随 2026-08-03「在案后续工作全清批」落地**（见上方首条）。验证：**124 lib + 9 gui** 全绿、clippy
  --all-targets 0、i18n 407 键 0/0/0、双 exe（gui 34967155 / cli
  26888658）。

- **v0.14.0 RELEASED（2026-08-03）**——自 v0.13.0 后的全部 15 提交：
  修饰超时 600s（8f62b8a）、相机基调批次（b1cf7ce）、Reset=刚打开状态
  （d1f6986）、LR 对标缺陷修复批（f0f96b4+c604477）、分析链超时重校准
  （c111ed0）、手感批（12611db+1807631）、自动镜头校正（d54de4b+
  debde8f）、GUI UX 批（b8ff358+7d823eb）+ 各批 ROADMAP 文档提交。
  发版细节（tag、资产字节）见 git tag v0.14.0 与 GitHub Release 页。

- **GUI 信息架构打磨批（UX batch，2026-08-03，已提交未推送）**——用户报
  "GUI 还是有些乱"，4 区域 UX 盘点工作流（toolbar/develop-panel/retouch-
  tools/visual-consistency，44.8 万 tokens，~80 项分级 findings 全带行号）
  后拍板"工具栏瘦身+归位"。落地：
  1. **工具栏单行化**：第二行整行删除——导出设置六件（格式/长边/锐化/
     质量/色彩空间/AI 降噪）迁入显影面板尾部新 **Export 折叠节**（质量
     滑杆常驻仅禁用=不再回流；AI 降噪 tooltip 补"批量渲染不含"），工具栏
     Export/Download 按钮 hover 动态回显当前交付摘要（export_summary：
     "JPEG · 2560 px · q95 · sRGB"）；AI 三件（Direction 输入框/Refine/
     Style 滑杆）迁入面板顶部 **AI 区**（原 AI verdict 节扩展，一次分析
     的全部输入输出一处齐）；批量进度条迁状态栏（原插在工具栏行首，
     等待时把全部控件右推 160px）；"Autoshop" 字标删除（标题栏已有）；
     Open 忙时禁用（原静默无操作）；Save XMP→**Save develop**（真相：
     非 RAW 根本不写 XMP）；"Export → ./out"→"Export"；撤销/重做 tooltip
     补说明；⿲(U+2FF2 CJK 字形,无 CJK 字体即豆腐块)→◫；⚙ 改开关语义；
     分组用 add_space（separator 换行成孤儿竖线）。
  2. **显影面板重排**（LR 顺序）：AI → 直方图 → Tone & WB → Presence →
     Curves → HSL → Grading →〔分隔〕→ Detail →〔几何〕Lens → **Crop**
     （镜头校正重定义画幅，故 Lens 在前）→〔分隔〕→ Masks → Versions →
     Export；节间 add_space(6)；Grading 的 Blending/Balance 加"全部区域"
     scope 标注（原读作"shadow Blending"）。
  3. **工具武装根治**：新 `disarm_tools()`/`tool_armed()` 取代 **14 处
     手抄互斥列表**（历史上两个真 bug 都来自抄本漂移；瞬时手势锚
     place_start/crop_drag/mask_drag/paint_last 一并归属）；
     resync_recipe_display 全量禁武装（Reset/undo 不再留半武装工具）；
     **range_picking 随蒙版选中变化清除**（原选中换行后取色仍写旧蒙版）；
     **进入图章不再清空画笔涂抹**（原 clear_mask 无撤销地毁掉为填充/修复
     画的区域；Clear 按钮独占清空职责）；放置类按钮（线性/径向/Redraw）
     改 selectable 武装态+可点击取消（原武装后面板毫无变化）；画布提示
     武装时 **ACCENT 常规对比度**（原 .weak().small() 与帮助文字同级）+
     全部armed 提示补"· Esc 退出"+ 径向放置不再被叫"渐变"+画笔提示改
     "Brush —"前缀（原与闲置态同读"After —"）。
  4. **退出确认层安全**：Esc=取消、Enter=全存后退（原快捷键块被 gate 完
     全失灵、备忘单却承诺 Esc）；按钮重排 [Cancel]…[Discard(警示色)]
     [Save all(PILL)]（原三个同样式按钮毁灭键居中）；列表滚动化+显示
     父目录/文件名（原裸 stem 截断 8 个，同名文件不可分辨）；标题
     "● Unsaved edits" 与状态栏 ● 呼应。
  5. **视觉一致性**：克隆源十字 PILL→ACCENT（双层色规自我违反——金色
     在暖色画面上消失）；修饰区 6 个按钮（含 2 个付费 API、2 个毁灭性
     像素操作）补齐 tooltip；蒙版 🗑 补 tooltip；命名规范："Noise Red."→
     "Noise Reduction"、"brush"→"Brush size"（并入 house slider=获双击
     重置+↑/↓ 微调）、两个"Clear"→"Clear crop"/"Clear brush"、蒙版
     "Temp/Tint"→"Temp shift/Tint shift"（相对量≠全局开尔文）、分级区域
     Title Case（Shadows/Midtones/Highlights/Global）、"＋ Radial"→
     "＋ Radial gradient"、🖊→⎘ 图章双钮同形。
  i18n：+40 新键 −26 死键（审计 407 调用点 0 漏译/0 漂移/0 重复）。
  Codex 只读复审 3 条**全修**（其余检查项全 clean，含 python 块交换无
  丢码、resync 全禁武装的四个调用点逐一判定安全、放置按钮互切正确）：
  ①退出层 Enter 全局吞键会盖过持焦的 Cancel/Discard→仅无控件持焦时触发；
  ②Export 节 Format+Long edge 同行两组合框中文下溢出 320px 面板→一行
  一项；③状态栏批量进度条排在 ● unsaved 前会在窄窗把标记挤出→标记
  最前。验证：**124 lib + 9 gui** 全绿、clippy --all-targets 0、终版
  gui exe 34957433（cli 26875258 字节不变——纯 GUI）。真机验收点：工具栏一行放下；
  导出设置在 Export 节且按钮 hover 显摘要；AI 三件在 AI 区；节序 LR 化；
  武装任何工具有 ACCENT 提示+Esc 可退；退出确认层 Esc/Enter 可用。
  记录未做（盘点在案）：变体条重构/状态栏上下文组/WB Temp 常驻/版本
  时间戳/直方图占位/长帮助文demote/Color-Colour 拼写统一/设置窗标题
  层级/全res三勾合一/工具条画布快捷键(K/W)。

- **自动镜头校正（A 档立项落地，2026-08-03，已提交未推送）**——三批之三。
  技术侦察结论（改变方案）：**Sony ARW 每张自带机内校正三件套**（0x7032
  暗角 / 0x7035 CA / 0x7037 畸变，各 16 节点样条，真机 DSC08276 实测齐
  全）——**不需要 lensfun 数据库**，逐张即本镜头在本焦距/光圈的厂商精确
  profile。公式实证（RawTherapee `rtengine/lensmetadata.cc` 实现 +
  stannum.io/blog/0PwljB 逆向互证，量级自洽）：暗角 gain=1/2^(0.5−
  2^(v·2⁻¹³−1))（v=0→恰 1.0）、畸变 factor=v·2⁻¹⁴+1、CA=v·2⁻²¹+1（R/B
  乘在绿通道映射上）；节点位 (i+0.5)/(n−1)，线性插值。用户拍板：**新照
  默认三项全开**（与机内 JPEG 观感一致）。架构：
  1. **[src/lensmeta.rs](../src/lensmeta.rs)**（新模块）：rawler
     GenericTiffReader 提取 + 换算为引擎空间 f32 因子；计数守卫（畸形→
     该分量缺失，绝不报错）；真值单测（A7RIV 实测向量：角落暗角 1.4249/
     畸变 0.9478）+ 常驻真机探针 probe_real_lens_metadata（env 门控）。
  2. **recipe.LensProfile**（引擎专用，不进 XMP）：三数据组+三开关；旧档
     缺字段→默认全关→**逐字节不变**（硬契约，测钉）；is_noop 视"如盖戳
     态"（全部可用分量开）为校准非编辑——用户拨开关=真编辑，存档/导航
     全程存活；clamp 防御手编档（增益 0.25-4/因子 0.7-1.3/CA 0.98-1.02）。
  3. **引擎**：暗角=apply_develop 0a 段线性光增益（16 节点径向 LUT，与
     手动暗角同骨架）；几何=apply_lens_geometry **一次重采样**合成
     profile 样条（Stannum 填幅 s=边缘 max g）+手动量+CA 分通道（R/G/B
     各自半径采样，非 CA 快路径单采样）；lens_geom_norm/lens_ungeom_norm
     正/逆映射（逆=上升前缀二分——复合映射在强手动桶形下过峰回折，域限
     制到峰前，与 undistort_norm 的 u_max 同语义）。
  4. **盖戳三端**：produce_recipe 尾部 photo_lens_profile（saved-first：
     有存档用其 verbatim 含旧档全关与用户开关，否则 fresh_lens_profile
     全开盖戳）；GUI 开照（OpenedBase 三元组+photo_lens 应用态+新照/仅
     XMP 恢复盖戳，recipe.json 原样）、Reset 重盖（烘焙变体空）、粘贴按
     目标重派生（源的校正数据对异镜头无意义）、分析 refine 剥/结果盖、
     烘焙变体画布剥；web：404/fresh-base 双端点带 lens_profile+客户端
     Reset 同义；GUI 批量渲染无存档 RAW 补戳。
  5. **GUI**：Lens 面板顶部"机内镜头校正"三开关（数据缺失灰禁；legacy
     恢复后拨开→从 photo_lens 回填数据）；蒙版/裁剪坐标映射
     (view_norm_to_orig/orig_norm_to_view/geom_to_view) 全量换 LensArg
     （profile+手动量）走复合映射——蒙版把手/覆盖层/画笔栅格在校正后画
     面上不漂移；ensure_mask_tex 变换键含 profile 开关。
  验证：**124 lib（+4：lensmeta 真值/序列化契约/引擎复合与往返/LRU 载
  荷）+ 9 gui** 全绿、clippy --all-targets 0、i18n 390 键 0/0/0、真机
  lensmeta 探针 PASS（16 节点×3）。Codex 只读复审 5 条**全修**：①web
  预览从不做几何（旧账——手动畸变/拉直也从未进 web 预览面板）→
  api_develop 补齐引擎几何链（镜头几何→拉直，不裁剪与 GUI 预览同策）；
  ②画笔覆盖层经 RGB16 路径 **alpha 被打平**（拉直旧账，profile 默认开
  使其必现：整画布红纱）→ 新 `apply_lens_geometry_rgba` /
  `rotate_straighten_rgba` 保 alpha 双胞胎（框外采样=透明）；③变换键
  粒度 geometry_active 布尔无法区分畸变/CA 组合→键改 profile 畸变
  专项布尔（CA 不移动覆盖层）；④基调估计在未校正中性上匹配已校正的
  机内 JPEG=角落提亮被全局曲线二次吸收→新 `render::estimation_base`
  （暗角校正后 ≤1MP 中性）统一 GUI/pipeline/serve 三处估计基准；
  ⑤is_as_stamped 只查 ca_r——半损档（有红无蓝）误判为盖戳态→改查
  CA 数据对。终版双 exe（gui 34936997 / cli 26875258）。真机验收点：开新 ARW 畸变/暗角/CA 即校正（对比机内 JPEG
  构图应一致）；Lens 面板三开关即时生效；旧存档观感不变；蒙版在开校正
  后的画面上位置正确。未做（记录）：非 Sony 机型（Fuji 0xf00b 等）、
  去紫边 de-fringe、lensfun 数据库路线（Sony 元数据已覆盖用户全部机
  身）。

- **LR 对标·手感批（2026-08-03，已提交未推送）**——用户拍板三批之第二批
  （缺陷→**手感**→A 档自动镜头校正）。纯 GUI（引擎/CLI 零改动，cli exe
  字节不变）。4 组：
  1. **滑杆手感**（`slider_impl` 重构 + `SliderFeel` 类别）：悬停 + ↑/↓
     微调（Shift ×10，LR 方向键语法；←/→ 仍是图库走图；以"无控件持焦"
     为门，文本框打字永不误触）；拖拽步进量化——整数域（±100 族/0-150/
     色相°）snap 1 显示 0 位小数（web 端曾被 "13.4849996…" 撑爆值框的
     浮点噪声根治），EV 类 0.01 拖/0.1 微调，0..1 类 0.001/0.01，Temp(K)
     对数轨微调 ≈当前值 1%（固定步长在对数轨一端亚像素另一端过大）；
     新 `slider_fine`（拉直°：0.1 步进——宽域整数 snap 会毁 0.1° 拉平）。
  2. **快捷键**：R=进/出裁剪（与按钮同禁武装工具）、[ / ]=笔刷大小
     （仅画笔/克隆启用时消费键）、Tab=隐藏/显示两侧栏（LR 语法；会话内
     状态，有意不持久化——重启不见图库读作坏了）；速查表 18→22 行。
  3. **裁剪手感**：边中点把手 4 个（自由比例只动该边；锁定比例按 LR 边
     把手语义在垂直轴居中重导出另一维，越界收缩）；**框外拖拽=旋转拉直**
     （绕框心屏幕空间 atan2，y-down 顺时针正=引擎顺时针°，图像跟手转，
     crop_drag 携起始角）；比例补 4:3/3:4/5:4/4:5（7→11 项）；光标
     语义补边把手/旋转；面板与画布提示改述（含 R 键）。
  4. **HSL 8-band 同屏**：波段下拉（一次只见一个 band）改 LR 混色器布局
     ——Hue/Saturation/Luminance 页签 × 8 波段行全显 + 每行色标
     （HSL_SWATCH 展示用波段中心色；引擎数学不动）；hsl_band 字段改
     hsl_tab。
  验证：120 lib + 9 gui 绿、clippy --all-targets 0、i18n 383 键 0/0/0。
  Codex 只读复审 6 条**全实证全修**（含 egui 源码引证）：①旋转 atan2
  ±180° 支切未归一→跨切 2° 被读成 −358° 撞夹（wrap 到 (−180,180]）；
  ②Frac snap 0.001 被 egui fixed_decimals(2) 的**存值取整**吃掉→snap 改
  0.01 与显示位一致；③Tab 的 egui 焦点遍历在 update 前已定向、consume
  拦不住→defocus_next 下帧 surrender_focus（否则首个控件持焦杀死全部
  快捷键）；④Ctrl+Z 中途撤销后 crop_drag 旧锚回写→resync 清 crop_drag；
  ⑤确认退出层背后 R/Tab/[ ] 仍活→快捷键门加 !confirm_quit；⑥微调直赋
  绕过 egui 取整（13.485→14.485 隐藏精度）→按类小数位 round。终版
  gui exe 34683663（cli 26641585 字节不变）。真机验收点：悬停滑杆 ↑/↓
  微调且整数域显示整数；R/Tab/[ ] 生效且 Tab 不吃后续快捷键；裁剪拖边、
  框外转（跨 180° 不跳）、4:3 可选；HSL 8 行同屏。未做（记录）：颜色
  分级 2D 色轮、TAT 图上拖拽调整、面板记忆折叠。——用户真机报分析回退启发式基线：「AI vision unavailable:
  http transport: …/v1/responses: Network Error: timed out reading
  response」。回退与披露本身按设计工作（heuristic.rs 带原因兜底）；根因
  与 8f62b8a（images/edits 300→600s）同类：`/responses` 各调用点预算按
  前推理模型时代校准（提案 120s / 风格描述 90s / 验证 60s），直连
  api.openai.com 跑推理级视觉模型（高细节图 + strict schema）的健康请求
  被客户端中途掐死。修复（[src/advisor/mod.rs](../src/advisor/mod.rs)）：
  ①预算常量化重校准 `PROPOSE=360s / STYLE=240s / VERIFY=180s`（类别注释
  记录实证依据；`AUTOSHOP_HTTP_TIMEOUT_SECS` 仍可全局覆盖）；②中央
  `transport_error` 助手——"timed out" 传输错误一律追加可操作提示（默认
  时限数值 + 覆盖旋钮），三个调用点（openai.rs 提案/describe_style、
  openai_verify.rs）统一换用；③顺藤修出同类更重隐患：**retouch.rs
  `detect_spots`（AI 去瑕疵 auto）用的是裸 `ureq::post`——完全无超时**，
  端点僵死会把 worker 永久卡住而 busy 锁死全 GUI——改走
  `post_with_timeout`（共享 PROPOSE 预算+覆盖）+ 同款超时提示。验证：
  120 lib + 9 gui 绿、clippy --all-targets 0、双 exe（gui 34678011 /
  cli 26641585）。真机验收点：重跑分析应不再 120s 掐断（慢也等到 6 分
  钟）；再超时的报错会写明时限与旋钮。

- **LR 对标·缺陷修复批（2026-08-03，已提交未推送）**——用户拍板三批打磨
  （缺陷修复 → 手感 → A 档立项=自动镜头校正）之第一批；问题源=4 区域 LR
  对标盘点工作流（tone-color/geometry-detail/local-retouch/workflow-ux，
  62.4 万 tokens 实证，锚点逐条 file:line）。7 项全落地：
  1. **曲线端点钉扎**（[src/render.rs](../src/render.rs) `curve_lut` +
     eval.rs 同规则）：曲线未自带端点时补 (0,0)/(1,1)——原先 interp 在首末
     点外夹平，**在曲线上点一下整图变常数灰**（实测复现）；显式端点仍权威
     （提黑曲线不受影响）；AI 产曲线同受益；eval 判分改与渲染同语义。行为
     变化=无端点的旧曲线渲染改变（这正是修复本身，已测钉）。
  2. **Before=画布配方的基调**（gui `set_before` + `before_curve` 字段 +
     update 每帧惰性刷新）：Before 面板/hold-B 原显示无基调中性显影，比
     After 起点暗 0.63~1.42 EV；现新照对比亮基调、旧存档按原调、烘焙变体
     原样，Reset/粘贴/恢复改曲线即自动重建（一次 LUT develop）。
  3. **直方图真相**：256 bin（每 8-bit 码值一格）+ 四通道**共享**纵轴归一
     （原每通道各自归一→相对高度无意义）+ 三角指示阈值对齐 J 覆盖
     （≤1/≥254，原极端 bin 跨 0-3/252-255 会对近削波亮警）；build_preview
     里"judged on EXPORT pixels"谎言注释改为如实（8-bit 预览值、缩放可
     平均掉小面积高光溢出=记录残差）。
  4. **锐化半径随分辨率**（V2_PLAN §4c 落地）：σ=clamp(0.0008·min(w,h),
     0.7,2.0)→box 半径（1280 预览=1，≥约1900 及全尺寸导出=2）；原硬编码
     1px 使预览与 61MP 导出结构性不同。NR 依 §4d 像素尺度**不变**（噪声粒
     度在传感器像素尺度），预览/导出观感差=披露注释。行为变化=大图带锐化
     导出字节改变。
  5. **径向蒙版 feather 补全**：GUI 边缘羽化滑杆 + 内外翻转开关（两者引擎
     一直渲染却均无控件，放置硬编码 0.5）；XMP **双向换算**——写方 ×100
     （旧写方把 0.5 原样给 LR 的 0..100 域=硬边）、读方 >1→÷100、≤1 按旧
     自写保真（LR 真值 Feather="1" 角例记录于注释）；新测试钉双向。
  6. **像素修饰可撤销**：undo 三栈改 `UndoStep{recipe, base(Arc), origin}`
     ——heal/clone/生成填充烘基图现在是一步可撤销（committed 按 Arc 指针
     身份比对；undo/redo 恢复变体像素 + refresh_active_pixels 重建
     before/画布/overlay）。内存取舍：每次修饰在历史里多留一份预览分辨率
     基图（100 步上限自然回收）。
  7. **web**：新 `/api/fresh-base` 端点 + Reset=刚打开观感（与 GUI d1f6986
     同义；旧服务器/非 RAW 回退保曲线）；9 处中文残留英文化（页面英文-
     only：AI 去瑕疵按钮/状态、修图/分析/图像节标题、popover 三库名）。
  验证：**120 lib（+2 新）+ 9 gui** 全绿、clippy --all-targets 0、i18n
  378 键 0/0/0。Codex 只读复审 4 条：同帧撤销竞态（HIGH，修=修饰落地帧
  `commit_now` 即时入史）、sel_mask 越界（修=resync 过滤，同号异蒙版残留
  记录）、曲线重复 x（注释钉"先到先得"）、LR Feather="1"=1% 角例（记录
  取舍——≤1 判旧自写保真优先）。终版双 exe（gui 34673242 / cli
  26636565，`c604477`）。真机验收点：曲线单点不再毁图；Before 与开照同
  亮度；修饰后 Ctrl+Z 能退；径向蒙版可调羽化。

- **Reset=刚打开状态 + 旧存档提示（2026-08-02，`d1f6986`，已提交未推送）**
  ——真机验收报"点了重置，还是暗的"。根因：用户已编辑照片几乎都有基调批次
  **之前**的旧存档（硬契约=缺 base_curve 字段→空曲线→按旧样渲染），而
  Reset 沿用"保画布曲线"语义——旧存档上曲线本来就是空，重置后依旧暗。修复
  （[src/bin/gui.rs](../src/bin/gui.rs) + [src/bin/i18n.rs](../src/bin/i18n.rs)，
  +41/−7）：①开照 worker 估计的基调节点存入新应用态字段 `photo_knots`；
  ②Reset 改为**重盖 photo_knots**（=本照片刚打开的观感：滑杆中性 + 相机
  基调；烘焙变体仍空曲线——其像素已带相机观感）；③打开旧存档照片加一行
  状态提示（"保存于相机基调功能之前——按原样渲染；Reset 可切换到相机基调"，
  EN/ZH）；④Reset tooltip 改述新语义。链路自洽：旧存档上 Reset→Ctrl+S =
  is_noop→中性存档删边车→照片回"新照"，下次打开自动盖戳（用户迁移旧照的
  一键路径）。已知残差：**web Reset 仍保画布曲线**（web 打开旧存档同样暗，
  无提示——待下批）。验证：118 lib + 9 gui 绿、clippy 0、i18n 375 键
  0/0/0、双 exe 重建（gui 34667456 / cli 26645626）。

- **相机基调批次：打开 RAW 不再暗 + 修饰不再切换像素源（2026-08-02，用户
  拍板"根治: 基调+统一"）**——用户报"每次加载 raw 都很暗，点 AI 修瑕疵就恢复
  正常亮度"。根因两层：①中性显影无任何相机基调（rawler 线性→sRGB，真机定标
  比机内 JPEG 暗 +0.63~+1.42 EV，且形状是 S 曲线非单一增益——中间调动 ~3×
  于趾部）；②heal/clone/生成填充非全分辨率基图曾是**相机内嵌 JPEG**，修完
  InPlace 烘焙进画布 = 像素源切换假装"恢复亮度"，且后续编辑/导出落在 8-bit
  相机曲线像素上。方案（[src/recipe.rs](../src/recipe.rs) `base_curve` +
  [src/render.rs](../src/render.rs)）：
  1. **recipe.base_curve 携带式相机基调曲线**（引擎专用，不进 XMP）：分位数
     锚点 luma-CDF 匹配（x=Q_neutral(p), y=Q_camera(p)，p 网格止于 0.98，
     (0,0)/(1,1) 钉扎；两侧对称 ≤1024px 缩略+全像素 1024-bin 直方图；同 bin
     分位数均值合并；近恒等→空）。`build_tone_lut` 把它复合在用户色调**之下**
     （final(x)=user(base(x))，LR 的 profile-then-sliders 次序）。**旧存档
     缺字段→空曲线→逐字节按旧样渲染**（硬契约）；~~fit 产物有意不带曲线（自洽
     完整解）~~——该契约 v0.24.0 作废（R15）、v0.25.0 定形（R16）：中性起解烧光
     模型容量是反推暗闷的根因，fit 现把标定**复合进求解闭环**（fit_recipe_from
     + pipeline::calibration_recipe，产物按构造携带标定；CLI match 保留内嵌
     预览+后盖戳契约 stamp_fit_calibration——「当前状态」v0.25.0/v0.24.0 条）；
     is_noop/●判定（新 `dirty_vs`）忽略曲线——校准非编辑。
  2. **盖戳权威 = pipeline::produce_recipe**（saved-first：有 recipe.json 用
     其曲线原样含 legacy 空，否则 photo_base_knots 新估计）——GUI/web/CLI 三端
     同一权威；GUI 开照（新照+仅 XMP 恢复盖戳，recipe.json 原样）、web 404/
     XMP 路径带节点（fresh_base_knots 复用 develop_base 缓存）、粘贴每目标
     解析（烘焙目标清空/有存档用其曲线/否则承源）、GUI 批量渲染无存档 RAW 补
     戳、GUI/web Reset 保画布曲线（GUI Reset 语义已被上条 `d1f6986` 取代为
     重盖开照节点）、烘焙变体上 Analyze 画布剥曲线（持久化仍全）。
  3. **修饰基图统一**：heal/clone_stamp/生成填充 RAW 基图一律引擎中性显影
     （full_res 全尺寸，否则 ≤2048px 缩略）——修完与画布同色调链，亮度跳变
     根治。代价：非全分辨率修饰也要 demosaic（clone 从即时变数秒）——已记录。
  4. **decode::embedded_preview**：相机自带渲染三级提取（preview→thumbnail→
     full_image）。rawler 0.7.2 源码验证：ARW 解码器**只实现 full_image**
     （JPEGInterchangeFormat 内嵌全尺寸 JPEG 提取，绝非 develop；默认实现
     Ok(None)）——A7RIV 全靠第三级；decode.rs 两处"full raw render"旧注释
     已纠正。
  5. 审查：对抗工作流 26 代理（4 维度 find + 逐条对抗 verify）22 findings
     （14 confirmed 含 2 BLOCKER：估计器直方图空洞→~30 级色调断层/暗片高光
     整段钉白（已用真实代码复现）、中性清存后 ● 永久点亮、CLI 写方 recipe
     永久钉暗、粘贴无 is_raw 门等）+ Codex 复审 5 条（烘焙变体双重上调、web
     新照暗突变、步进混叠、同 x 取低偏置、双 develop 成本）——全部修复或记录。
  6. 有据取舍：analyze-after-fit 继承 fit 的空曲线（saved-first 一致性优先）；
     auto/batch/GUI 批量对无存档 RAW 多付一次 develop（正确性优先，已披露
     warning）；web retouch 面板残差；Reset 在变体上保画布曲线语义。
  验证：**118 lib + 9 gui** 全绿（+5 新测试：复合/端点钉扎、空洞无平台无钉白、
  尖峰均值合并、旧档序列化/is_noop/clamp、LRU 载荷）+ 常驻真机探针
  `probe_real_raw_base_look`（AUTOSHOP_PROBE_RAW 门控：DSC08530.ARW median
  中性 0.326 → 带基调 **0.497** vs 相机预览 **0.495**）；clippy 0；i18n 374
  键 0/0/0。真机验收点：打开任意新 RAW 应即近相机 JPEG 观感；点修瑕疵前后
  亮度不再跳；旧已保存显影观感不变；Reset 不再变暗。

- **修饰(images/edits)超时预算重校准（2026-08-02）**——用户真机报
  「修饰失败: transport: …/images/edits: Network Error: timed out reading
  response」。根因：直连 api.openai.com（image_provider=api，非 8317 桥）
  跑 gpt-image-2 + quality=high + 像素预算默认 8 294 400（API 上限），
  生成时长超过 [src/generative.rs](../src/generative.rs) 里按 gpt-image-1
  时代（60-120s）校准的 300s 总预算——正常成功中的请求被客户端掐断。
  修复：预算常量化 `IMAGES_EDIT_TIMEOUT_SECS = 600`（每次 400-重试各享
  全额），transport 报错含 "timed out" 时附加可操作提示（默认时限数值 +
  `AUTOSHOP_HTTP_TIMEOUT_SECS` 覆盖 / 降 `AUTOSHOP_IMAGE_QUALITY` /
  `AUTOSHOP_IMAGE_MAX_PX` 提速）。诊断副产物（环境事实，未改代码）：
  中央 `%LOCALAPPDATA%/autoshop/autoshop.local.json` 尚不存在，实际生效
  的是 cwd 回退的 `D:/Projects/Autoshop/autoshop.local.json`（设计内：
  下次在设置 UI 保存即写中央文件）；OPENAI_API_KEY 现存在于 .env 与
  用户级环境变量。

- **第二轮 debug + UI 打磨批次（2026-07-25，已提交未发布——待真机验收）**
  ——用户令"再进行一遍debug+UI前端打磨"（中途不弹窗）。方法：6 维度工作流
  （store 集成/GUI 状态机/GUI 打磨/web/双语文案/跨端契约）+ 按维度对抗验证
  + 完整性批判 → 61 确认 + 4 批判补充，去重后 ~35 项全部处置；Codex 复审
  再出 6 条（3 MAJOR）全修。要点：
  1. **备份门下沉 lib**（store::backup_saved_develop）：复制式快照（工作
     recipe 不动，可在操作前打快照）+ **栅格版本化**（v<N>.<name>.png 冻结
     引用，快照不再被后续 fit 覆写栅格改写观感；复制失败=整个备份失败并
     回滚——绝不假称有备份）；三端统一：web /api/analyze 与 GUI 同门（拒
     写时 200+warning），CLI 侧 StyleIndex::save 拒绝空索引覆盖好索引；
     delete_version 连冻结栅格一起清。
  2. **GUI 数据安全**：Fitted 带 persisted 旗标（备份门拒绝时不再假清
     ● 基线/nav_stash）；zoned fit 快照提到分割前+门拒绝时跳过 zoned
     （保护已保存栅格）；fit 的 XMP 失败不再吞掉已成功的 recipe 写入；
     Analyze 拆分 recipe/XMP 写（基线只随 recipe）+失败 toast；粘贴到
     打开照片全成功后同步基线（假 ● 修）；save_version 从磁盘取号（缓存
     失刷不再覆写自动备份）；**关窗拦截**（✕ 有未保存→应用内 保存全部/
     放弃/取消 层，不弹系统窗）；crop 拖拽/Clear 置 dirty（直方图/削波
     不再滞后 + 在飞帧丢弃后重派）；resync 清 range_picking/placing_mask
     等武装工具索引；AI-select 去重按栅格名（stem 前缀感知）并复活旧引用。
  3. **GUI 打磨**：AI verdict/rationale 移到显影面板顶部（Accept 平静色/
     其余警示色）；设置窗新增「显影库」区（根路径展示 + **从旧 ./out 导入
     显影**按钮→worker 迁移全图库）；版本行 🗑 删除；方向键走图自动滚动到
     选中缩略图；Temp(K) 对数刻度；Tone & WB 补 ● 活动点；蒙版重命名缓冲
     （一次改名=一步 undo，跨行安全提交）；Heal 全分辨率 tooltip/质量下拉
     悬停+译；Download… 过 guard_readonly；缩略图缓存并入 store_root
     （AUTOSHOP_DATA_DIR 生效）；设置窗标题/模型状态/Before 标签等全译。
  4. **web**（子代理）：分析结果串照片根治（id 守卫）+selectPhoto
     latest-wins；非 ASCII 文件名不再 panic（RFC5987）；损坏 recipe 422+
     前端兜底；web Reset+Save=删四处存档（与 GUI 同义）；blob URL 回收；
     文本框内 Ctrl+Z 不再回滚配方；分析/保存后徽章即时刷新；popover 展示
     显影库路径+风格索引实际文件。
  5. **migrate 加固**：进程内 memo（打开不再每次全枚举 ./out）+ legacy
     多根探测（env AUTOSHOP_LEGACY_OUT → cwd → exe 目录，升级用户换目录
     启动不再"丢"旧编辑）+ 显式导入 API。
  6. **文案真相**：所有仍称显影态在 ./out 的字符串（GUI 5 处/CLI help 与
     -o 尾注/web popover/README/ARCHITECTURE --bare 陈旧调用）全部改为显影
     库表述；ZH 修正：变体≠版本、双击恢复默认值（非归零）、局部蒙版、
     全分辨率（非全画幅）、gitignored 措辞、sidecar/边车保留给 XMP。
  基线 **113 lib + 9 gui**（+2 serve 编码测试）、clippy 0、i18n 374 键
  0/0/0。已知取舍：memo 使"会话中途出现的 legacy 文件"延至下次进程或
  手动导入（文档化）；web 陈旧未保存分析提示只改文案不缓存提案；
  serve `image_oauth_supported` 字段保持 false（web 端确实不可配置）。

- **边车中央库批次（2026-07-24 实现，已随 v0.13.0 发布 2026-07-25）**——
  用户拍板"中央库+路径键"后实现。显影**状态**（recipe.json / <stem>.xmp /
  v<N> 快照 / 蒙版栅格）从 cwd 相对 ./out 的裸 stem 键迁至
  `store::store_root()`（`AUTOSHOP_DATA_DIR` env → `%LOCALAPPDATA%/autoshop`
  → temp）下 `develops/<stem>-<fnv1a64(绝对路径小写)>/`，同名异目录照片
  不再互踩、换目录启动也能找到编辑；**导出成品图（developed/retouch/heal/
  clone/preview/matched/style.txt）留在 ./out**（交付物）。要点：
  1. 新模块 `src/store.rs`：路径键（FNV-1a 64 手写钉死——DefaultHasher 跨
     Rust 版本不稳，会孤儿化整库）、`recipe_target/xmp_target/version_target/
     raster_target/style_index_path/settings_path`、`has_develop`（中央∨
     legacy 存在性，供 badge/web 列表/CLI batch）、`migrate_legacy`（打开
     即迁移：解析 recipe 连带栅格搬家并把引用改写成裸名；解析失败原字节
     搬（Unreadable 契约保持响亮）；跨卷 rename 失败退 copy+delete；央本
     已存在则不覆盖）、`resolve/relativize_mask_paths`（读时锚定 recipe
     所在目录/写时收敛裸名——recipe 内栅格引用相对化，显影目录可搬迁）。
  2. pipeline：`write_recipe` 默认写央本+落盘前 relativize 副本+source.txt
     面包屑；`xmp_target` 重指向央本（全部调用点自动跟随）；`guard_readonly`
     放行 store root；风格索引读取央本→legacy 回退。
  3. GUI：read_saved_develop 顶部 migrate_legacy + 央本→legacy 双读（kind
     注明 "(legacy ./out)"）+ resolve；backup_saved_develop 比较前先
     resolve（否则带栅格的重写永远"不同"→假快照）；版本快照/列表走
     store；中性存档删除**四处**（央本+legacy 的 recipe+xmp，legacy 残留
     会经回退复活"已清除"的编辑）；批渲染读央本→legacy；badge=
     has_develop；分割/反推栅格写入显影目录。
  4. Web/CLI：/api/recipe 迁移+双读；/api/develop|export|download 渲染前
     resolve（api_recipe 原样透传裸名）；风格索引构建/信息走央本；
     `match --zoned` 栅格+canonical 走 store（fit_zoned 不建目录，调用点
     补 ensure_parent）；`apply` 按 recipe 所在目录 resolve；`batch` 的
     pending 判定=has_develop（旧库不再被重复计费重分析）。settings
     （autoshop.local.json）移央本+旧 cwd 文件读回退。
  5. 测试 111 lib（+4：photo_key 消歧/FNV 参考向量/resolve-relativize
     往返/migrate 全套）+9 gui（sidecar 优先级测试加 legacy 迁移断言）。
  Codex 只读复审 9 条：**3 条已修**（迁移改"先复制栅格→tmp+rename 发布
  recipe→成功后才删旧"失败全回滚；backup_saved_develop 改 Result——快照
  失败则 AI 分析/反推**拒绝覆盖**显式保存并 toast/note 说明；write_recipe
  改 tmp+rename 发布，崩溃不再留半截真源 JSON）；6 条记录为取舍——并发
  迁移竞态（同源幂等，见 migrate_legacy doc）、中断迁移（逐文件可续跑）、
  相对 AUTOSHOP_DATA_DIR（高级用户显式覆盖自担）、路径拼写身份（symlink/
  UNC 不 canonicalize——其自身失败模式更多，source.txt 供诊断）、64 位
  FNV 碰撞（需 stem+哈希同撞，个人库量级 ~1e-10）、非 UTF-8 stem lossy
  （Windows 全 Unicode，风险极低）。其余已知取舍：迁移静默成功（状态一致
  无需打扰；失败时 kind 带 "(legacy ./out)" 可见）；serve.rs
  `out/imported` 上传目录留 ./out（属输入暂存）；fit_zoned 单测仍写测试
  自有的 ./out 文件（测试专用路径，非生产写点）。

- **全量 debug + 协同性审计批次（2026-07-24，已随 v0.12.0 发布）**——用户令
  "全量debug+打磨使用体验，同时检查各个功能之间的协同性"+报"蒙版按钮
  点击变移动"。方法：6 维度并行审计工作流（指针状态机/异步预览/边车
  持久化/蒙版渲染语义/偏好流程/跨面协同）+ 按维度对抗验证 → **77 项
  findings 全部 CONFIRMED**；修复分四批（gui.rs 本体串行保协同性；
  render.rs、serve.rs+web、pipeline/main/advisor/segment 三组互不相交
  文件由隔离子代理执行）。基线 **107 lib + 9 gui**（+5 新测试），
  clippy(--all-targets --features gui -D warnings) 0。要点：
  1. **蒙版行点击=选择恢复**（用户报症）：整行曾包在 dnd_drag_source
     里（drag 交互层吃掉点击、微动即浮行）→ ☰ 专用把手当唯一拖拽源，
     行主体纯 selectable_label，drop 落点区=整行（gui.rs 蒙版列表）。
  2. **持久化统一契约**：recipe.json = 任意源类型的唯一真源（Ctrl+S 对
     PNG/TIFF 也写 recipe.json，仅 XMP 限 RAW）；中性配方存档=删除边车
     （清 ● 徽章）；程序性写入者（AI 分析、反推）覆盖前自动把既有存档
     备份为 v<N>（backup_saved_develop；显式 Ctrl+S 直接覆盖）；GUI 分析
     现与 CLI/web 一样持久化；web /api/xmp 双写 + /api/recipe 回退 XMP；
     CLI match 补写规范边车、analyze -o 时 XMP 跟随同目录。损坏的
     recipe.json 走 SavedDevelop::Unreadable **响亮降级**（toast+状态），
     中性边车 NoopOnly 不再谎称"已恢复"。
  3. **未保存保护**：saved_recipe 基线 + 状态栏 ● 未保存标记 + 导航
     nav_stash（切照片把未保存画布按路径暂存本会话、回来即恢复并提示）；
     批渲染对打开中的照片改用活配方（与 paste 同规）。
  4. **指针/工具状态机**：Esc/中断后残留 crop_drag/mask_drag/paint_last
     全部清理（无拖拽即无抓取原则）；快速抓取用 press_origin 命中（egui
     drag_started 有 ~6px 阈值）；小蒙版把手最近者优先取边缘；删除/拖放
     /⬆⬇ 后 range_picking/placing_mask 索引统一经 remap_mask_indices
     重映射；Paint 复选框补全模式互斥；画笔支持单击点涂+双击缩放在工具
     态被禁；削波层以当前旗标为准（在飞帧不再覆盖 J 切换）；AI 区域框
     常显（不再被工具/把手悬停藏掉）。
  5. **引擎蒙版语义**（render.rs，子代理）：位图栅格缺失＋invert 曾整幅
     全强度应用→现整体跳过（含 coverage 一致）；feather=0 椭圆边界 NaN
     （黑点）→ ramp 钳制硬边；roundness 实测无法定契约（用户 160 个 LR
     边车 201 个径向全 0）→ 显式记录为 no-op + 钉死测试。反推 sky 栅格
     改名 mask-zone-sky（GUI+CLI），不再与「AI 选天空」的 mask-sky 相撞
     互删；重跑分割去重同栅格蒙版；批量粘贴剥离位图蒙版（带提示）。
  6. **Web 协同**（serve.rs+index.html，子代理）：预览与导出统一解码源
     （develop_base 缓存 + 构建互斥锁）；latest-wins 序号防旧帧回流；
     500 带真实错误文本（tiny_http Drop 空响应根因）；空风格索引拒绝
     覆盖好索引。
  7. **杂项 UX**：B 对比/Space 平移焦点门（打字不再闪原图）；⚙ 重点击
     不再清空表单；模型拉取中重开设置不丢 in-flight；保存后 key 提示按
     解析后配置（env key 不再误报"未设置"）；提供商切换换默认模型；预览
     worker panic 不再误清 busy；变体条 busy 时点击给 toast；保存/版本
     失败走 toast；启动语 i18n；多文件拖放提示；直方图/削波按裁剪窗计算
     （与导出像素契约一致）；straighten 下径向真实轮廓多边形 + 画笔
     overlay 过同一几何变换；XMP 读回"Autoshop N"占位名视为未命名；分割
     蒙版持久名固定英文键（数据不随语言漂移）；F1 双击说明改"重置为默认
     值"；README 图像 OAuth 描述纠正；python sidecar 输出改捕获、错误尾
     部随报错透出（lib.rs sidecar_tail）。
  已知取舍/遗留（勿当 bug 重报）：边车按裸 stem 键控在 ./out（同名异
  目录照片相撞）与 out/settings 的 cwd 相对性=磁盘布局设计题，**用户已
  拍板（2026-07-24）：中央库+路径键**（%LOCALAPPDATA%/autoshop/，按照片
  绝对路径消歧，./out 只留导出成品图，打开时自动迁移旧边车），作为
  v0.12.0 之后的独立批次实现、真机验收后另发；radial 边缘把手在
  straighten 下的手感（引擎正确，显示已用多边形，拖拽轴映射保持原
  语义）；>24MP 缩略图门排队策略未动；web 预览 8-bit 降采样基底与导出
  的残余差异已在 serve.rs 注释记录；badge 仍为存在性检查（开销权衡，
  异常态由打开路径 toast 补偿）；pipeline 融合后复验仍 fatal（子代理
  记录）。

- **AI 分析认证修复（2026-07-17，已随 v0.11.2 发布）**——v0.11.1 过了
  信任门后用户再报 "分析失败: claude exited Some(1): "（冒号后空白）。
  三层根因（全部当日实测复现，`src/advisor/claude.rs`）：
  1. **`--bare` 语义变化**：本机 `claude` CLI 已自动升级 2.1.158→2.1.210，
     新版 `--bare` 的 --help 明文 "Anthropic auth is strictly
     ANTHROPIC_API_KEY or apiKeyHelper via --settings (OAuth and keychain
     are never read)"——即 `--bare` 下验证器**永远不可能**走用户订阅
     OAuth（OAuth 凭据本身健康且自动刷新正常）。A/B 实测：同一调用
     带 `--bare` 报 "Not logged in"（无 key）或 400（有 key）；去掉
     `--bare` 即成功。修复：换成 `--setting-sources "" --strict-mcp-config
     --disable-slash-commands` 三旗标——实测 0 插件启用、0 hook 注册、
     无用户技能、stderr 干净（保住当年 `--bare` 的隔离目的），OAuth 可用。
  2. **环境里的 `ANTHROPIC_API_KEY` 抢占计费**：Windows 用户级环境变量
     里的 key 被 headless claude 优先采用（即使不带 `--bare`、即使用户
     在交互模式里拒绝过该 key），把计费导向余额为空的 API console →
     400 "Credit balance is too low"。修复：spawn 前
     `cmd.env_remove("ANTHROPIC_API_KEY")`（该 provider 设计即 OAuth）。
  3. **错误吞没**：headless claude 失败时通常 stderr 为空、真实错误在
     stdout JSON 信封（`is_error:true, result:"…"`），而 CliFailed 只带
     stderr → 用户只看到 "claude exited Some(1): " 空白。修复：失败路径
     先解析 stdout 信封透出 `result`；解析不出且 stderr 为空时带上
     stdout 前 300 字符。
  注意：此修复要求 `claude` CLI 支持这三个旗标（≥2.1.x；CLI 默认自动
  更新）。机器侧删除用户级 `ANTHROPIC_API_KEY` **救不了** v0.11.1 及更早
  的 exe（`--bare` 下会变成 "Not logged in"）——唯一根治在代码侧。

- **AI 分析信任门修复（2026-07-14，已随 v0.11.1 发布）**——用户报
  "分析失败: claude exited Some(1): Ignoring 3 permissions.allow entries
  from .claude/settings.json: this workspace has not been trusted"。
  根因：`ClaudeProvider::verify` 直接 `Command::output()`，子进程继承
  GUI/CLI 的 cwd；headless `claude` 把 cwd 当工作区，若该目录带
  `.claude/settings.json` 且从未交互接受过信任对话，CLI 报错并退出 1。
  实测复现（2026-07-14）：同一调用在 `D:/Projects/Autoshop` 下 stderr
  逐字节出现该信任报错、在全新 temp 目录下无报错。修复：spawn 前
  `cmd.current_dir(std::env::temp_dir())`（验证器纯 stdin/stdout，
  不触碰文件，temp 目录恒存在且无工作区设置）。全库唯一 `claude`
  spawn 点即此处（`src/advisor/claude.rs`；`denoise.rs`/`segment.rs`
  的 spawn 是 Python sidecar，无关）。

- **性能批次 #3-B：引擎并行化 + 全量 backlog 清零（2026-07-12，已随 v0.11.0 发布 → `e3a4096`）**
  ——把 v0.10.0 记录的 29 项审计 backlog **全部**落地（`b15996c` 引擎 +
  `e2b4b6b` GUI + `73b4f43` CLI/Web/Style，后者由两个 worktree 隔离子代理
  实现、diff 回传合并）。release 基准探针（钉死校验和）**通过**且大幅加速：
  单彩色蒙版 81→44.7 ms/帧、天空+地景对 149→34.5、无彩对 92→25.0
  （相对 v0.8.1 同机基线）。
  1. **引擎 rayon 并行（render.rs，`b15996c`）**：rawler demosaic 本就并行，
     尾部全串行——现在 tone/RGB曲线/HSL/分级/饱和/dehaze/暗角/双蒙版通道/
     unsharp/NR/u16·u8 打包/f32 转换/旋转·畸变重采样全部按行或按像素并行。
     逐像素数学未动 → 逐位不变（模糊通道保持每列运算次序）。新依赖
     rayon（本就在 Cargo.lock 里，rawler 用）+ bytemuck（零拷贝转换）。
  2. **powf 热环 LUT 化**：dehaze/暗角每像素 6-7 次 powf → 共享 4096 项
     传递曲线 LUT 对（同 v0.8.1 色偏增益已验证的亚量化误差包络）+ 暗角
     径向增益 LUT；dehaze 的 airlight 直方图**保留精确 powf**（避免估计
     跨 bin 漂移）。convert_export_color_space：u16 输入 → 65536 项**精确**
     解码表（逐位不变）+ 按值传递（sRGB 恒等与 16 位路径改移动，省 ~366MB
     克隆）+ 行并行。
  3. **内存/访存**：box_blur_v 改行主序 + 每列 running-sum（原列主序在导出
     尺寸平面上全程打到 DRAM 延迟；逐位不变）；orient_f32 bytemuck 零拷贝
     （竖构图 61MP 原付三次 ~732MB 拷贝）；rotate/distort 借用已是 Rgb16 的
     源不再克隆；打开时中性 tone-pass 短路（原对全传感器无条件跑恒等 LUT）。
  4. **GUI（`e2b4b6b`）**：削波开关（J/▲/直方图三角）改用保留的上一帧像素
     即时重建 overlay（原每次开关整幅重显影 100-300ms）；RGB→RGBA 纹理
     转换移入 worker（PreviewDone.after，UI 线程只 tex.set）；**持久缩略图
     磁盘缓存**（%LOCALAPPDATA%/autoshop/thumbs，键=路径+mtime+大小哈希，
     二次会话 ~1ms 回显，>10k 文件按龄一次性后台修剪）；>24MP 烘焙图解码
     单许可门（原 6 并发 60MP TIFF 峰值 ~2GB）；图库纹理超 1500 项按视口
     窗逐出（原 1 万图钉 ~0.7GB）；蒙版覆盖栅格上限 1024px（display-only）。
  5. **CLI/Web/Style（`73b4f43`）**：CLI batch 3 线程有界池（网络与渲染重叠，
     --limit 语义/失败语义/汇总保持，stdout 行原子）；Web /api/develop 与
     preview/thumb 共享 (path,mtime) 键解码缓存（每滑杆手势不再全幅解码）；
     风格索引 4 线程池按槽位写回（exemplar 次序与串行完全一致）。
  基线 **102 lib + 9 gui** 全绿、clippy 0、探针校验和不变。已知偏差（诚实
  记录）：dehaze/暗角像素环走 LUT（8-bit 输出在量化内不变、16-bit 可 ±数
  LSB，同 v0.8.1 色偏先例）；打开路径中性 tone-pass 跳过消除了旧恒等 LUT
  的舍入噪声（更正确，非逐位同旧版）；覆盖 overlay 超 1024px 用降采样参考。
- **边车恢复 + 响应性快赢（2026-07-12，已随 v0.10.0 发布 → `c312a9f`）**——用户报
  "图库显示 ● edited 但打开后不加载 XMP"。根因：徽标查 ./out 边车
  （recipe.json‖xmp，gui.rs `gallery_panel`），而打开路径从不读它们
  （`Msg::Opened` 新开分支一律 `EditRecipe::default()`）；且 GUI 自己的两条
  保存路径（Save XMP 按钮、反推 worker）只写 XMP——位图蒙版/重着色增益
  根本进不了经典 XMP，反推结果关掉即丢。三层根修（`6957897` lib + `3124596` gui）：
  1. **XMP 读取器 `xmp::xmp_to_recipe`（xmp.rs，与写入器同文件）**：逐字段反演
     `recipe_to_xmp`——全局滑杆/HSL/分级/曲线/裁剪拉直/暗角畸变/参数化蒙版
     （线性+径向，含范围蒙版；位图蒙版与写入器同规则跳过）。**溯源规则**：
     As-Shot 的 Temperature/Tint 是相机值不是编辑，仅 `WhiteBalance="Custom"`
     才导入（Tint 另认我们自己的 `x:xmptk="Autoshop"` 标记）；LR 恒写的
     2 点恒等主曲线折叠为空。局部滑杆刻度精确反演（曝光 /4→×4 二的幂无损；
     ×100 后 4 位小数吸附回 UI 网格）。round-trip 性质测试钉死：
     recipe→XMP→recipe 对写入器舍入安全值全等。`crs_f32` 迁至 xmp.rs
     （eval.rs `pub(crate) use` 转出，style.rs 路径不变）。
  2. **打开即恢复（gui.rs `read_saved_develop`）**：新开分支优先
     `<stem>.recipe.json`（无损：位图蒙版/color_gains/role 全回）、缺则 XMP
     反演；无效/中性边车恢复无物（`is_noop` 门）。undo 基线=恢复后配方，
     「重置」可回中性；状态栏 "ready — restored saved edits ({kind})…"。
  3. **保存改无损**：Save XMP (Ctrl+S) 同时写 recipe.json；反推 worker 对
     **任意**源持久化 recipe.json（RAW 另写 XMP 不变）——反推结果首次可
     关闭→重开完整还原。
  **响应性快赢（同批 `3124596`，源自 63-agent 对抗验证审计的 29 项确证）**：
  ① "● edited" 徽标每可见行每帧 2 次文件 stat → 按索引缓存
  （`edited_badge`，换目录/本 app 写边车时失效）；② 解码底图 LRU
  （`base_cache` 4 项，path+edge+mtime 键）——图库来回挑片二次打开跳过
  整幅 demosaic；③ 范围蒙版 overlay 的 masks-cleared 参考重建（UI 线程整幅
  显影，2560/4096 达 100-300ms）在指针按住期间挂起、松手后一帧内补建
  （几何蒙版不受影响）；④ 涂抹蒙版纹理改 `TextureHandle::set` 原地更新
  （原每笔一帧新建纹理）；⑤ `build_preview` `into_rgb8` 移动缓冲
  （原 to_rgb8 每 tick 深拷贝 ~3.3MB）；⑥ 目录扫描 `DirEntry::file_type()`
  免每文件二次 stat（符号链接回退 `Path::is_dir` 保持行为）。
  基线 **102 lib + 9 gui** 全绿（+5 xmp round-trip、+1 边车优先级、
  +1 LRU）、clippy(gui) 零警告。待用户真机验收：打开带 ● edited 的照片应
  直接回到保存的编辑；反推→关→重开应完整还原（含分区蒙版）。
  ~~引擎性能批次 #3-B backlog~~ → **已全部随 v0.11.0 落地**（见上一条；
  唯一形态调整：打开路径的"降采样后显影"以更安全的等价方案落地——中性
  tone-pass 短路 + 全管线并行化，预览像素不做线性/伽马域缩放置换）。
- **GUI 多语言 i18n：英文骨架 + 中文切换（2026-07-11，已随 v0.9.0 发布 → `ca6f73e`）**
  ——把原生 GUI ~430 条中英混排硬编码文案统一到零依赖、英文即键的翻译层。
  发布前 4 路对抗审计（密钥/范围/键覆盖/MaskRole）0 blocker，键覆盖审计报出
  3 条工具提示缺中文（WB 吸管 hover / 裁剪提示 / 镜头提示）——已补全并逐字节
  核对键在 gui.rs 调用点与 i18n.rs 目录两侧各出现一次（否则中文模式静默回退英文）。
  1. **新模块 `src/bin/i18n.rs`（`cad6c68`，557 行）**：`Lang{En,Zh}`（Copy+serde，
     默认 En）、`tr(lang,en)`（En 原样返回=骨架；Zh 查 `ZH_ENTRIES` 缺则回退 en）、
     `trf(lang,en,&[(name,val)])`（运行时 `{name}` 替换——`format!` 要编译期字面量，
     翻译串只能运行时插值）。`ZH_ENTRIES` = 唯一译文目录 = 语言版本控制单一来源。
     键必须与调用点英文字面量逐字节一致，否则 Zh 静默 miss（编译不报、测试也过）。
  2. **gui.rs 全量路由**：~430 条用户可见字面量过 `tr`/`trf`；每渲染函数顶
     `let lang = self.lang;`（Lang: Copy，不借 self，避开 worker 闭包借用冲突）。
     `Prefs.lang`（serde 容器级 default → 旧存档缺字段解码为 En，不重置其他偏好）、
     `AutoshopApp.lang`、save/restore 已接；设置区 Language 下拉切换下一帧即生效。
  3. **MaskRole 蒙版名解耦（recipe.rs/fit_zoned.rs）**：`enum MaskRole{#[default]
     Custom,ZoneSky,ZoneLand}` 挂 `LocalAdjustment.role`（`#[serde(default)]`），把
     分区蒙版身份从可翻译显示名剥离——名字可翻译而不破相等判断与 recipe.json 往返。
     engine-only，**不进 XMP**（xmp.rs 零引用，Bitmap 蒙版被写入器整体跳过，已验）。
     3 处 zoned 测试从 `m.name==` 迁到 `m.role==`。旧 recipe.json 缺 role 解码为
     Custom；新写 recipe 被更旧 build 读会因 `deny_unknown_fields` 报错（前向不兼容，
     app 内部数据、无 XMP 影响，可接受，同 color_gains 先例）。
  4. **Cargo.toml `autobins=false`**：两 `[[bin]]` 均显式声明，使 `src/bin/i18n.rs`
     （无 main）作 gui.rs 子模块而非独立二进制目标。
  基线 **97 lib + 7 gui** 全绿、clippy(gui) 零警告、双 exe 重建（gui 33451827 /
  cli 25974719）。范围仅原生 GUI——Web UI（index.html/serve.rs）、CLI（main.rs）未
  动。待用户 GUI 真机复测手感（英文默认 → 设置切中文 → 全 UI 中文；缺译回退英文）。
- **性能批次 #2-C：预览卡顿根治（2026-07-10，已随 v0.8.1 发布 → `ce69f27`）**——用户报
  "处理图片时会有些卡"。多代理只读剖析 + 无头基准定位两层根因，各根修：
  1. **色偏增益 LUT 化（`759c9ca`，render.rs）**：v0.8 分区蒙版的
     `color_gains` 在 apply_wb / apply_masks 里逐像素逐通道跑两次
     sRGB↔线性 `powf`——1280×853 生产型基准实测单天空蒙版 613ms/帧、
     天空+地景对 1208ms（同蒙版去掉色偏仅 53/92ms，证明幂运算占 ~90%）。
     根修：把精确线性光增益编成每通道 4096 项 LUT（复用色调阶段同款采样器），
     单蒙版降到 81ms、对降到 149ms，且 8-bit 预览**逐字节不变**（基准校验和
     不变；LUT 插值误差 <1.5e-5 < 1/255 量化，`colour_gain_lut_matches_the_
     exact_linear_light_formula` 钉死）。附 `preview_mask_perf_probe`
     (#[ignore] release-only 机器相对基准，带校验和防"跳步取胜")。
  2. **预览异步 latest-wins（`c1e8b8d`，gui.rs）**：display 原来同步跑在
     egui `update()` 里，整帧构建把 UI 冻住（2560/4096 100-300ms，带 v0.8
     色偏蒙版 0.6-1.2s）。改单后台 worker：`build_preview` 引擎显影+几何+
     单次 rgb8 转换（喂直方图/削波/缩略图）离开 UI 线程；完成回调丢弃
     (base,recipe) 已变的陈旧帧，快拖自动合并到 worker 吞吐、指针 60fps 不卡。
     Arc 共享 base 像素（派发 O(1) 非 50MB 深拷贝）、纹理 `TextureHandle::set`
     原地更新（不再每 tick 新建纹理管理项）、蒙版 overlay coverage-aware key
     （局部效果滑杆改"做什么"不改"作用范围"→不重建整帧覆盖栅格；纯几何蒙版
     不再跑第二次 masks-cleared 显影，仅范围蒙版保留）。顺带修复：蒙版"反转"
     复选框 `Response.changed()` 被丢弃（切换只改配方不重渲染）。无头测试
     `async_develop_discards_stale_frames_latest_wins` +
     `overlay_skips_rebuild_for_local_effect_sliders`（egui::Context::default，
     不 run_native）。基线 **97 lib + 7 gui**，clippy(gui) 零警告，双 exe 重建。
     待用户 GUI 真机复测手感（尤其 2560/4096 拖滑杆、带天空/地景双蒙版反推）。
- **反馈批次 #2-B：语义分区反推（2026-07-09/10 夜，随 v0.8.0 发布）**
  ——跨"区域性观感 vs 全局滑杆"表达力鸿沟的正路，全程 fail-first +
  真机对（_DSC9621 × reimagine-5）驱动迭代 4 轮渲染目视：
  1. **引擎局部 temp/tint（`d58ca60`，render.rs）**：LocalAdjustment 自 v1
     就带 Temp/Tint 且 XMP 会导出，但引擎从不渲染（GUI 蒙版滑杆拖了没反
     应的既有缺口）。`local_temp_to_kelvin`（相对 ±100 → mired 线性 ∓80
     围绕 5500K 锚，≈半张 CTO/CTB，render.rs）+ apply_masks 每蒙版一次
     `wb_gains`、线性光逐像素、WB→tone→sat 镜像全局次序。fail-first 红→
     绿 + 满帧蒙版≈全局 WB 等价性测试钉死映射与 tint 符号。
  2. **color_gains 重着色增益（`9c55e24`，recipe.rs/render.rs/fit_zoned.rs）**
     ——实测出的模型上限：调色板移植（蓝天→金天）要求线性 r/b ≈5.3×，而
     **任何** WB 参数化（扫满 2000–40000K 黑体）封顶 ≈1.9×、±100 饱和只
     ×2——Temp/Tint/Sat 物理上画不出重绘。新字段
     `LocalAdjustment.color_gains: Option<[f32;3]>`（线性光逐通道增益，
     0.05..8 钳制，中性收敛回 None；引擎专用——经典 ACR 无对应物，本来就
     只挂在 XMP 会跳过的 Bitmap 蒙版上），apply_masks 与 WB 增益乘法合成。
     可识别性论证：全局 cast 曲线必须重门槛是因为"哪里"未知；蒙版回答了
     "哪里"，区上逐通道增益就是可识别的——这正是表达力升级本身。
  3. **fit_zoned.rs 新模块（`9c55e24`+`a5173b2`+`09172f2`，~700 行）**：
     zone_moments（蒙版加权线性光一阶矩）→ fit_zone_dials（增益=want/2^EV
     精确闭式）→ `fit_recipe_zoned` 编排：全局 fit 先行 → `segment_file`
     ×2（源+目标各一次天空分割）→ **天空区 + 地景区**（同一栅格
     `inverted=true` 复用——第一轮真机渲染的教训：只修天空留下蓝晕带贴着
     金天空）→ 每区独立验收。分割/依赖/退化任何失败 → 优雅回退纯全局
     fit + rationale 注记，绝不报错。
  4. **分区验收哲学（`09172f2`，真机实测驱动）**：帧全局 look_err 会按构
     造否决正确的分区重绘（实测：天空区矩 0.507→0.016 落点几乎精确，帧
     全局却 0.1768→0.1792——生成式目标的天空占比 8% vs 23% 构图不同 +
     蓝→金迁移带质量被 worst-band 色相项读成伤害）。分区 do-no-harm 裁判
     = **区内矩误差**（zone_err ≤50% 原值）+ 帧全局仅作**有界漂移保险**
     （±0.02，实测漂移 +0.0024）。非对称占比回归测试钉死该几何。
  5. **区内色调 CDF 求解（`09172f2`）**：线性均值匹配后地景仍读起来暗很
     多（目标地景=日照台地+深峡谷阴影，亮像素统治线性均值、感知跟随分
     布）。区内加权 luma CDF → quantile 映射 → 复用 `fit::fit_tone_sliders`
     （同全局 stage-1 基底+幅度先验）解 6 个局部色调滑杆；**可识别性守卫**
     （实测）：近单值源区（平雾天空 IQR<0.05）上 quantile 映射退化（解出
     EV −0.70、区残差 0.016→0.108 倒退）→ 回退矩-EV、色调保持平。
  6. **接线（`b78daeb`）**：CLI `match --zoned`（蒙版落 GUI 约定
     out/<stem>.mask-sky.png）；GUI `zoned_fit` Pref（eframe 持久化，默认
     ON——有优雅回退）、Settings「反推」区开关、start_fit 分支 + 完成注记；
     XMP 诚实注记由构造完成（rationale 进 sidecar 注释 xmp.rs:350，Bitmap
     蒙版被跳过 xmp.rs:79）。
  7. **真机 v4 验收（无头 CLI + 渲染目视 + 数值）**：双分区 attach（天空
     0.507→0.016、地景 0.151→0.006）；天空奶金 [0.69 0.60 0.48]（目标上
     天空 [0.63 0.56 0.50]）、地景暖红棕有结构、无蓝晕、无 re-hue；地景
     亮度较目标仍差 ~0.1 sRGB——目标重打光了构图（诚实残余，rationale 有
     注记），且蒙版滑杆现已实时渲染，用户面板一拖即补。测试基线
     **96 lib + 5 gui**，clippy(gui) 零警告。**待用户 GUI 真机验收**。
- **反馈批次 #2-A：反推统计加固（2026-07-09 深夜，`7471d35`，本地未推送）**
  ——用户 v0.7.0 真机复测报"效果还是不好"（_DSC9621 × reimagine-5：全图刷
  成高饱和橙、天空由雾蓝变橙桃）。实测定位三个根因并全部 fail-first 修复
  （用户选定方向 C = 先统计加固后分区反推）：
  1. **旋转预算门（第三道 cast 门，fit.rs）**：目标是"调色板移植"级的全暖
     AI 渲染时，蓝通道 CDF 匹配把雾蓝天空整区 re-hue ~170° 进目标原生橙——
     外来色否决按设计不拦（落点是目标原生色相）、聚合门被全图通道均值改善
     抬过（实测 ratio 0.25）。新门做像素对齐旋转普查：两端都可见着色
     （chroma≥0.04）且色相移动 ≥75° 的像素占画面 ≥5% → 拒。阈值全部实测
     标定（雾霾修正 75° 处 ≈0、紫峡谷 112°、金天空 ~170°）；书面代价：重
     色偏校正若需 >75° 旋转也会被拒（保守失误可在显影面板补救，区域 re-hue
     不可救）。
  2. **色调证据对称化（`tone_cdf_pair`）**：中性门是"两侧同一人口"的可识别
     性假设——旧代码两侧独立决定，目标把淡色区 re-hue 出中性集时（雾蓝天空
     中性、金天空不中性），一侧中性 CDF 对一侧全像素 CDF，色调解算整体畸变
     （真机对里 Shadows −49 的诡异组合即此来源）。现成对判定：任一侧样本不
     足或中性份额比 >1.75× → 双侧一起回退全像素 CDF。
  3. **do-no-harm 终检**：饱和度是唯一按启发式（均值彩度追赶）拟合的旋钮且
     中途不可评判（正确的饱和会先放大潜在色偏，曲线阶段再清除——阶段局部验
     证门试过，砍死雾霾回归的 sat 被否决）；管线终点若整体 look_err 比不动
     还差则折半 sat 并重拟曲线。其诱因已被 #2 根治，现无 fixture 可达，作
     为保险留存（代码注释明示）。
  4. **诚实面**：rationale 新增三类注记（残差仍远 >0.12 时建议直接用 AI 变
     体或分区编辑 / sat 顶格 ±60 / 曲线因 re-hue 风险被扣），confidence 随
     诚实残差自然下降（真机对 0.73→0.25）。
  测试 82→85 lib：金天空策略回归（修前 sky 49°、Δ164° 失败）、**真机几何
  布线测试**（雾霾源×全暖目标，旋转门是唯一拒绝者——变异验证：摘门即败；
  峡谷合成对上 ratio 门冗余拒绝、看不见该变异）、旋转份额 pin（0.1× 余量
  同时从下方钉住 ROT_DEG）。真机对无头复验：曲线全扣、天空保持蓝、err
  0.275→0.177 诚实上报。附注：多代理对抗审查揪出 9 项实证问题全部处置
  （含两个变异验证的"未承重"洞、rationale 误导措辞、文档漂移、测试复制生
  产代码——审查代理曾误留 eprintln/stash 污染工作树，已恢复并全量复验）。
- **反馈批次 #1 → v0.7.0 已发布**（2026-07-09，tag `v0.7.0` → `7c36ee3`，
  双 exe 资产字节核对 33286921/25914296，标记 Latest）——用户真机报障
  "反推紫天空 + 扁平"（峡谷照截图对）+ 四项指令（解决问题/加去雾、修 bug、
  代码库 debug+优化、优化 UI）：
  1. **反推紫天空根治（`7b6a64c`，fit.rs）**：cast 曲线的聚合验收门对
     **跨带色相灾难结构性失明**（天空被染紫后质量落进目标为空的紫/品红带、
     又流出蓝带——双侧带权门把两边都跳过，色相项什么也看不见）。根治 =
     第二道**外来色否决**（`cast_paints_foreign_hues`）：有/无曲线两次渲染
     像素级对齐，曲线把 ≥5% 画面涂到离目标一切色相 ≥45°(1.5 ACR 带) 的
     色相上即拒。判别量是**色相距离**（实测：峡谷紫距目标 60°+、雾霾修正
     残差仅 5-40°——任何单一粒度的"色族成员"规则都会误判一边：±15° 细窗
     把雾霾修正判 15% 伤、整带份额把其橙黄裙边判幻影黄）。fail-first 复现
     用例 `warm_rock_cast_must_not_violet_the_pale_sky`（红偏移暖化——乘性
     暖化会被保色相的饱和度阶段吸收、根本不触发 cast 曲线）+ 判据 pin 测试
     `foreign_hue_veto_separates_haze_from_canyon`（峡谷 2× 余量触发、雾霾
     0.000 不触发）。
  2. **去雾引擎落地（`66062be`，render.rs）**：`dehaze` 字段原是**空壳**
     ——GUI 滑杆在、XMP 导 `crs:Dehaze`，但渲染管线从不读它。现为
     apply_develop 阶段 0b（暗角后、色调 LUT 前）：线性光散射反演
     `I=J·t+A(1−t)`，逐像素 min 通道当雾密度（**非**空间暗通道滤波——
     O(N) 且统计上 CDF 可辨识）、airlight=min 通道线性 P99 直方图（跨分辨率
     稳定）、单仿射保通道序（无品红/青反转）、v=A 定点保亮天、负值=凸组合
     加雾不裁剪。5 个测试（物理构造的雾夹具 t=0.55/A=0.9）。**反推刻意不加
     dehaze 阶段**：色调 CDF+饱和度之后其唯一残差特征（亮度-彩度联合轮廓）
     对生成式目标内容混淆，与已删的 per-band HSL 同类。
  3. **两个"永久卡死 busy"根因修复（`9b60a62`）**：① 4 个 AI ureq 调用
     **零超时**（默认 agent 无读取期限——桥挂/代理停摆 = 工作线程永久阻塞、
     GUI 全锁）→ `advisor::post_with_timeout`（connect 10s + 按延迟等级
     propose 120s / style 90s / verify 60s / images-edits 300s，
     `AUTOSHOP_HTTP_TIMEOUT_SECS` 全局覆盖）；② 工作线程**无 catch_unwind**
     （rawler/image 对坏文件 panic = 终结 Msg 永不到达）→
     `AutoshopApp::spawn_worker` 唯一 spawn 收口点，panic 合成该点位的失败
     Msg，15 个站点全部收编（fetch_models 的手写 RAII guard 被统一替代）。
     附 blur_plane 零维守卫（`Ord::clamp(0,w-1)` w=0 会 panic 的潜在类）。
  4. **滑杆流畅度 + chrome 打磨 + 快捷键速查（`24ed6a3`）**：拖动中显影
     **自适应合并**（1.5× 实测显影耗时，33-500ms 夹取；4096px 预览从每帧
     100-300ms 同步显影的卡顿降到 ~40% 占空比，松手帧立即显影不失真）+
     变体缩略图中拖跳过 + 位图蒙版 (path,mtime) 键进程级解码缓存（分割重跑
     覆写同文件，只按 path 会钉死旧蒙版）；画廊蓝/金双强调色统一为金 PILL
     系（**画布上工具覆盖层刻意留蓝**——金手柄在暖片上隐形，规则记录在
     常量处）；设置窗加滚动（保存键曾可能够不着）、状态栏截断+悬停全文、
     Export/Download/Save XMP/AI Analyze/Reset/Style/Fit 补 tooltip、面板
     标题双语化；**F1 / ? / ⌨ 快捷键速查表**（候选池项交付；O 蒙版覆盖层
     此前无任何可见控件）。
  基线 82 lib + 5 gui 测试、clippy(gui) 零警告、双 release 构建绿；无弹窗
  纪律全程遵守——真机验收（滑杆手感/紫天空实照重跑/去雾观感）待用户。
- **图像角色 OAuth 模式 / codex 桥（2026-07-09，`c389df6`，v0.6.0 发布）**：
  图像角色（vision 提配方 + 生成式 fill/heal/reimagine）新增 `image_provider`
  开关——**OAuth（本地 Codex 桥）** | **API（真 OpenAI key）**，与分析角色的
  OAuth|API **对称**。OAuth 模式经 CLIProxyAPI（`127.0.0.1:8317`，持有用户的
  ChatGPT 订阅令牌，上游 `chatgpt.com/backend-api`）走订阅出图，**无需 OpenAI
  key**；选 OAuth 自动填桥地址 `http://127.0.0.1:8317/v1`、API-Key 标签改
  「Gate token」、拉取模型按钮变可达性测试、image-gen 回退模型顺序按模式切换。
  纯 UI + 一个 config 字段（`config.rs` `image_provider`，缺省 `"api"` 保持旧
  行为；`image_is_oauth()` 判定；`gui.rs` `SettingsForm.image_provider_oauth`
  + 幂等自动填地址），**引擎零改动**——两种模式都落到同一 OpenAI 兼容 HTTP 路径。
  已知硬上限：订阅出图路径经 codex 内建 `image_gen` 工具，输出面积锁 ~1.57 MP
  （honors 宽高比、免费路不可提；全分辨率需真 OpenAI key 的 `images.request`
  scope）；遮罩编辑语义非像素保真但 `composite_region` 天然免疫。ToS 灰区、
  测试烧订阅额度——详见持久记忆 `autoshop-codex-bridge`。真机点击链未走（无弹
  窗纪律，靠编译 + 75 lib/5 gui 测试 + 源码走查验证）。
- **变体/版本条重构（2026-07-08，用户报障"AI 生图后再调整又变回去"+
  "图片版本没有可选的"，随 v0.6.0 发布）**：把"单一工作图 (src_path,
  base_preview, recipe)"模型换成**变体条**（Lightroom 虚拟副本 / Capture
  One 变体，**非合成图层**——它们不叠加，是同照片的平行版本）。一个变体 =
  (底片来源, 配方)：**原片**（底片=RAW，你的显影）/ **AI 生成**（底片=生成
  PNG 像素，观感烘焙其中）/ **反推**（底片=同一 RAW 中性，观感在配方 → 可
  编辑/导 XMP/出全分辨率）。底部缩略图条点击**无损切换**（各记各的底片+配方）；
  生成→自动新建「AI 生成」变体并切过去（编辑的就是生成图当底片，**不再变
  回去**）；反推→自动新建「反推」变体；fill/heal/clone→就地更新当前变体像素。
  **彻底移除** master/master_restyled/open_note/continue_from_master/「以此
  母版继续修图」整套绕过——`2fc9092` 的补丁被此结构性正解取代（各变体天然
  隔离，不可能二次烹饪）。关键正确性修正：反推的拟合底改用 `source_preview`
  （生成后 base_preview 已是生成图，拿它当底会把生成图拟合到自身≈中性）。
  UX：统一视觉主题（`install_theme`——PILL 金强调/圆角/间距/标题字号）。
  **对抗审查两轮 + 一次同步终审**（Workflow 多代理，各发现独立证伪）：
  第一轮 6 项确认全修（生成变体上 fill/heal/clone/导出/XMP 曾误用 src_path →
  统一 `active_source_path()` 变体感知像素源；`delete_variant` 补 busy 卫；
  retouch 结果按 `preview_edge` 烘焙不再降清晰度并重建 mask_paint；失败开图
  清工作态防 src_path↔变体错位；生成 origin 文件系统探测唯一命名防
  delete-then-reimagine 别名）；第二轮 2 项确认全修（就地修补后 repoint
  `variant.origin` 使导出/反推/续修跟随修补像素 = WYSIWYG；Download 建议名
  跟随 active_source_path）；终审 CLEAN。gui.rs ≈ +600/−200。测试 75 lib +
  5 gui、clippy 0、release、最小化烟雾均绿。已知边界（非本次引入）：修补过的
  原片变体导出 TIFF 含修补像素、但 XMP 是参数式无法承载像素修补，二者会分歧。
- **v0.5.2 已发布**（tag `v0.5.2` → `a57be95`，双 exe 资产字节核对
  33174745/25899129）：UX 批次5（`d987c5b` 顶栏换行不裁按钮+最小窗口/
  蒙版图上手柄编辑/放大后拖拽即平移）+ 反推配方根治（`6de045d` 下条明细）。
  注意用户指令（2026-07-07）：**调试时不弹窗**——引擎级改动跳过 GUI 启动
  烟雾，动了 gui.rs 才启动且最小化。
- **反推配方修复（2026-07-07，用户真机报障"反推 XMP 之后很奇怪"）**：
  紫天空/橄榄岩/滑杆顶死（Contrast −97、Shadows −100、红橙 hue +45）在
  用户的 _DSC9621 真对上逐位复现并分阶段渲染定位，三个根因全部根治
  （fit.rs）：①色调求解病态——近共线基底+名义岭（1e-4）让"巨大对冲"组合
  靠 ε 获胜，现 `TONE_PRIOR=0.02` 同时做岭和曝光扫描的选型惩罚；
  ②按带 HSL 拟合对非像素对齐的生成式目标**统计上不可辨识**（带心色相差
  把内容差异误读为旋转，13° 门限内"可信"证据整带旋转即成灾）——整级删除，
  按带意图归风格提示词路径（与局部蒙版同理）；③通道色偏曲线改**验证式
  接受**——均匀色偏（雾霾）一通道一映射是精确模型、内容差异则是错误模型，
  以带色相项的 look_err 降到 ≤0.85× 才保留（`CAST_ACCEPT_RATIO`）；
  look_err 本身补上**最差带**色相项（加权平均会让小面积的天空灾难隐身，
  实测混过验收门）。新回归 `hazy_to_clean_fit_stays_sane` 钉死：无退化
  滑杆、误差严格改善、拟合后每个有像素带的色相偏差 <15°。真对最终渲染
  蓝天+自然暖岩，置信度诚实降 0.80→0.43。测试基线 75 lib + 5 gui。
- **v0.5.1 已发布**（tag `v0.5.1` → `b92d4f3`，双 exe 资产字节核对
  33190058/25904350）：UX 四批 + debug 清扫整批（下条明细）。
- **v0.5.0 tag 之后的提交（2026-07-07 已 push，随 v0.5.1 发布）**：
  `763a2bc` UX批次1（蒙版覆盖叠加/削波警告/Esc）→ `eb6a098` 叠加参考缓存
  → `51c151d` UX批次2（hover 预览/直方图三角灯/批量进度条）→ `be60c52`
  UX批次3（蒙版⬆⬇排序/光标语言/缓存key收窄）→ `55e7e07` **debug 清扫**：
  ①方向统一——rawler ARW 内嵌预览不带 EXIF 转正（crate 源码实证），旧管线
  在 develop **之后**才 oriented() ⇒ 竖拍 RAW 的 crop/straighten 会错轴；
  现两侧都在最前端转正（引擎 `orient_f32` 复用同一 `oriented`，decode 端
  `preview_only`/`decode_raw` 同函数转正），Normal 方向逐位不变，回归
  测试+真 ARW 61MP 全流程实测；②hover_mask 改帧作用域（折叠面板/换图
  不再粘滞）→ `a494156` ROADMAP 交接刷新 → `4f16a8c` UX批次4（削波
  三角按通道显色/蒙版真拖拽排序/裁剪柄方向光标——候选清单清零）。
- **v0.5.0 已发布**（tag `v0.5.0` → `3ab41b6`，双 exe 资产字节核对）：
  三大项整批——C2 手动畸变 / D2 P3+AdobeRGB 真 gamut 导出 / A② AI
  主体天空分割（位图 mask 通路 + python sidecar，用户真机实测）。
- **v0.4.0 已发布**（tag `v0.4.0` → `e175bf8`）：范围蒙版 / 双轨续接 /
  导出管线 / 高分预览 / 暗角补偿 / sRGB ICC / 版本快照——A-G 整批。
- **~~C2 手动畸变校正~~ ✅ 完成（2026-07-06 深夜，见 §C，提交 b623e5a）**
  ——坐标映射整体设计（original→corrected→view 三空间合约）+ 引擎径向
  重映射 + GUI 全调用点接入 + XMP，67 lib + 4 gui 测试。
- **~~D2 P3/AdobeRGB 输出~~ ✅ 完成（2026-07-06 深夜，见 §D）**——真
  gamut 变换（色度推矩阵 + 双 TRC）+ CC0 profile 双件 + GUI 色彩空间
  下拉 + Prefs，69 lib + 4 gui 测试。
- **~~A② AI 主体/天空分割~~ ✅ 完成（2026-07-07 凌晨，见 §A）**——
  引擎位图 mask 通路（MaskGeometry::Bitmap + 双线性采样 + XMP 跳过）+
  `python/segment.py` sidecar（subject=rembg U²-Net / sky=SegFormer
  ADE20K，实测均通）+ GUI 两键入口，72 lib + 4 gui 测试。
  **← 天空后端已于 R27 Batch-4 换成 OneFormer ADE20K Swin-L（MIT）**：
  SegFormer 权重许可证只许「research or evaluation purposes only」，见
  「当前状态」R27 Batch-4 条 ⑧。下面各处 SegFormer / ~14MB 的字样是**当时的
  记录**，一律以那一条为准。
- **三大项至此全部触底。** 剩余工作 = 各节「未做/已知边界」小项（去紫边、
  Upright、lensfun、位图 overlay 半透明显示、tile 金字塔、水印等）+
  真机验收清单；无未开工的大工程。
- v0.3.0 → `fa9add8`，v0.2.0 → `1bc57ff`。
- **有序批次 ①-⑤ 全部完成**（详见各节 ✅ 小节，含实现锚点与已知近似）：
  ①曲线编辑器 ②批量复制/粘贴 ③WB 吸管（含 WB 预览前置重构）
  ④拉直（引擎真旋转+自动内接裁剪）⑤仿制图章（clone_raw 像素通路）。
- **差距批次 A① 亮度/颜色范围蒙版已完成**（见 §A ✅ 小节：recipe/render/
  xmp/gui/advisor 五层，60 lib + 4 gui 测试）。A②（主体/天空 AI 分割）
  未做——前置是引擎位图 mask 通路。
- **差距批次 B 双轨打通已完成**（见 §B ✅ 小节：母版路径入 GUI 态 +
  「⤴ 以此母版继续修图」保留配方续接）。
- **差距批次 F 导出管线已完成**（见 §F ✅ 小节：ExportOpts 长边/锐化/质量 +
  批量渲染 worker，61 lib 测试）。
- **差距批次 E 高分预览已完成**（见 §E ✅ 小节：1280/2560/4096 预览分辨率
  下拉，切换保配方重解码）。
- **差距批次 C 两片全部完成**（见 §C ✅ 小节）：暗角补偿（线性光域径向
  增益 + GUI 镜头校正区 + XMP VignetteAmount/Midpoint）+ C2 手动畸变校正
  （三空间坐标合约 + 引擎径向重映射 + GUI 映射链全接入 + XMP
  LensManualDistortionAmount，67 lib 测试）。
- **差距批次 D 第一步 导出嵌 sRGB ICC 已完成**（见 §D ◐ 小节：三格式
  显式编码器 + CC0 profile 入库，64 lib 测试）。
- **差距批次 G 版本快照已完成**（见 §G ✅ 小节：`<stem>.v<N>.recipe.json`
  编号快照 + 版本区存/载 UI）。
- 更早已上线：反推配方（`fit.rs` + CLI `match`）、gpt-image-2 弹性高分辨率
  （≤8.3MP + 400 回退）、风格提示词提取、GUI 生产化（直方图/toast/快捷键/
  拖拽/持久化/折叠分组/双击归零）。
- 待用户真机验收（v0.3.0 起累计）：曲线拖拽/吸管/图章/拉直/范围蒙版
  手感；「以此母版继续修图」链路（修补→动滑杆→再修补→导出）；导出长边/
  锐化/质量 + 批量渲染选中；预览 2560/4096 的滑杆延迟是否可接受；暗角
  补偿手感；版本快照存/载；导出 ICC 在广色域屏与真 LR 的显示；范围蒙版
  XMP 与 VignetteMidpoint 在真 Lightroom 打开的效果；持久化"正常关闭→
  重启恢复"；高分辨率生成与风格提示词的真实 API 行为（付费调用，有 400
  回退兜底）。**v0.5.0+UX 阶段新增待验**：AI 选主体/选天空按钮真机手感
  （GUI 内点击链路，sidecar 命令行已实测）；畸变滑杆与真 LR 同数值强度
  对比；P3/AdobeRGB 文件在广色域屏与印刷流程观感；蒙版覆盖叠加透明度
  （255,40,40 α≤140/255）与 hover 预览响应；削波三角灯灵敏度（任一像素
  即亮）与按通道显色可读性；批量进度条；蒙版拖拽排序手感（浮影/插入线，
  ⬆⬇ 按钮保留）；裁剪柄方向光标；**竖拍 ARW 全流程**（方向统一后
  显示/蒙版/裁剪/拉直应全部正确——修复靠单测+横拍实测，竖拍样张未过）。
  **v0.5.2 新增待验**：窄窗口下顶栏折行观感；蒙版手柄命中半径（12px）
  与拖拽手感；放大平移 vs Ctrl 框选切换顺手度；**反推配方在 GUI 内重跑**
  （引擎路径已在用户 _DSC9621 真对上复现→修复→复验，GUI 点击链路同函数）。
  **变体条重构新增待验（随 v0.6.0 发布）**：① 生成出片→底部出现「AI 生成」变体
  并自动切过去、微调滑杆不再变回原图；② 反推→出现「反推」变体（滑杆可编辑、
  RAW 写 XMP）；③ 缩略图条点击在 原片/AI 生成/反推 间无损来回切换；④ 停在
  「AI 生成」变体上 Export/Download **导出的是生成图像素**（非原片中性）、
  Save XMP 提示先反推；⑤ 在生成变体上 fill/heal/clone 修补的是生成图、且导出
  跟随修补（WYSIWYG）；⑥ × 删除非原片变体；⑦ 生成两次得两个独立「AI 生成」
  变体互不覆盖；⑧ 统一主题观感。真机点击全链未走（状态机经编译+75/5 测试+
  两轮多代理对抗审查+同步终审 CLEAN）。**#2-B 分区反推新增待验（GUI 链路）**：
  ① 新 build 里对 _DSC9621 × reimagine-5 重跑反推（Settings「反推」区开关
  默认 ON）——应出现「反推·天空」「反推·地景」两个蒙版、天空转奶金、无蓝晕
  无紫（CLI 无头链路 v4 已目视+数值验收，GUI 点击链路同函数）；② 地景若嫌
  暗，在蒙版面板拖「反推·地景」的 Exposure——蒙版滑杆现已实时渲染（顺带验
  Temp/Tint 从"仅 XMP"组移入实时区后拖动即时生效）；③ 首跑天空分割会下载
  ~~segformer-b0（~14MB，看状态栏提示）~~ **← R27 Batch-4 起为 OneFormer
  Swin-L（~880MB）**；python 依赖缺失时应静默回退纯全局
  反推且 rationale 有说明。

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
  所以 v0.29.x 以前竖拍 ARW 在显示/显影/导出全链都是横的。三个消费点
  （render.rs 渲染钩、decode.rs 的 Meta 尺寸 + 预览转正、`camera_rendition`）
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
  现行通道顺序 = **dehaze → 融合 WB+tone+sat → clarity → texture → NR**，
  每条各自 `!= 0.0` 门控（此前「只调这三项」的蒙版三重落空：不渲染、
  `engine_active` 判不活、栅格预算加载器连位图都不载）。dehaze 复用全局
  同一模型（`apply_dehaze` 拆出 `dehaze_airlight` + `dehaze_px`，拆分前后
  golden 位级一致），airlight 每帧只估一次且取自全局显影后的画面 ⇒ 蒙版
  叠放顺序不改变雾模型；clarity/texture = `unsharp_luma_weighted`（把权重
  乘到亮度差上，与「整幅滤波再按权混合」严格等价，故只需两个 f32 平面而
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

## ① 色调曲线交互编辑器（✅ 已完成）

- 数据已通：`recipe.tone_curve/red_curve/green_curve/blue_curve:
  Vec<CurvePoint{input,output: u8}>`（recipe.rs）；引擎组合方式——master 曲线
  在滑杆样条**之后**复合（`build_tone_lut` 末尾 `sample_lut(&curve, hermite_eval…)`），
  RGB 曲线在 master 之后（`apply_rgb_curves`）；分段线性 `interp`。
  XMP：`ToneCurvePV2012(+Red/Green/Blue)`（xmp.rs `curve_elem`）。
- GUI 设计：develop_panel 新 CollapsingHeader「曲线 · Curves」；通道选择
  （主/R/G/B）；自绘 widget：`allocate_exact_size(~边长 220)` + painter——
  网格、直方图背景（有 `self.histogram`）、曲线线条（按引擎同款 `interp`
  采样保真）、控制点拖拽（命中半径 ~8px）、空白处点击加点、拖出框外删点
  （LR 手势）、input 保持严格递增去重。改动 → `clamp()+dirty`。
- 无引擎改动。测试：曲线点排序/去重的纯函数可单测。

## ② 批量：配方复制 / 粘贴 / 同步（✅ 已完成）

- GUI：gallery 支持 Ctrl+点击多选（现 `selected: Option<usize>` 单选，加
  `HashSet<usize>`）；按钮「复制配方」/「粘贴到选中(N)」。
- 粘贴 = 对每张写 `write_recipe` + `write_xmp`（./out，RAW 才有 XMP），
  可选跳过 crop/straighten（LR 同步对话框的简化版：一个 checkbox）。
  worker 线程跑批 + 状态/toast 汇报；沿用 `Msg` 通道模式。
- 不渲染成品（可选 flag 后续加）；库只读不变。

## ③ WB 吸管（✅ 已完成，含前置）

- **前置已做**：新共享阶段 `apply_recipe_wb`（render.rs，apply_wb 旁）接入
  develop_preview / render_to_image / render_baked_to_image 三条路径；
  `temperature_k.is_some() || tint != 0` 即生效（修复了 tint 单独无效的旧坑）。
- 吸管已做：`render::solve_wb_from_neutral`（对数网格扫 K 使 r≈b，绿残差
  解析出 tint，与 `wb_gains` 同一正向模型）；GUI 色调区「💧 吸管」按钮 +
  图上点击取 5×5 均值（取 base_preview 的 pre-develop 像素）。
  单测：合成偏色像素 → 反解中和（<2% 残差）+ 预览 WB 生效性。

## ④ 拉直（✅ 已完成）

- 引擎：`render::rotate_straighten`（顺时针、双线性、16-bit）+ 公开的
  `render::inscribed_dims`（闭式最大内接矩形），在两条导出路径的用户裁剪
  **之前**、orientation 之后应用；GUI `redevelop` 用同一函数旋转预览。
- 坐标空间约定（重要）：`recipe.crop` 存**拉直后**空间（导出旋转后裁剪，
  裁剪工具无需映射）；masks/画笔/吸管/region 存**原始**空间——gui.rs 的
  `view_norm_to_orig / orig_norm_to_view / geom_to_view` 在数据边界换算
  （共用引擎 inscribed_dims，0° 恒等，roundtrip 有单测）。
- 已知近似（待真 LR 验证）：angle≠0 且带 crop 时 XMP 的 CropLeft…/CropAngle
  组合语义与我们的"先转后裁"是否逐像素一致未对照过真实 ACR 边车。

## ⑤ 仿制图章（✅ 已完成）

- 引擎：`HealSpot.clone_raw`（跳过 heal 的边界色调匹配 = 原样搬运 + 羽化）
  + `retouch::clone_stamp(src, mask, source_norm, full_res, out)`——涂抹 blob
  → spots，每个 spot 的供体偏移 = 源点 − blob 中心（PS 非对齐取样）。
- GUI：Retouch「仿制图章」节——进入图章模式，Alt+点击取源（十字标记，
  存原始帧坐标），共用画笔涂目标，「⎘ 克隆已涂区域」worker → ./out
  像素母版（同 heal，非 XMP）。单测锁定 clone（原样）vs heal（色调匹配）
  的语义差异。
- 已知近似：拉直角≠0 时画笔 overlay 纹理按原始帧直贴（落点计算正确，
  显示未旋转）——heal/clone/fill 共同的显示级问题，engine 结果不受影响。

## 与 Photoshop 的核心差距（2026-07-06 调查 · ①-⑤ 完成后）

> 定位前提：目标是"日常出片替代"（LR/ACR + PS 修图子集），不是 PS 的
> 设计/合成全集。按对日常出片的影响排序；「现状」均为当日代码实测。

### A. 智能选区 / 范围蒙版（① ✅ 2026-07-06 · ② ✅ 2026-07-07）
- PS/LR：Select Subject / Sky、亮度/颜色范围蒙版。
- **① 亮度/颜色范围蒙版 ✅**：五层打通，权重 = 几何 × 范围（相交）。
  - recipe.rs：`RangeMask` 枚举（Luminance 4 数梯形 = ACR LumRange 原样；
    Color = 参考色 rgb + amount 容差 + px/py 取样点）+
    `LocalAdjustment.range: Option<RangeMask>`（serde default，旧 JSON 兼容）；
    clamp 强制梯形非降序。
  - render.rs：`range_weight`（亮度=梯形 ramp，退化边=阶跃；颜色=亮度不变
    色度距离，除以各自 luma 后欧氏距离，d_max = 0.15+0.9·amount）；
    apply_masks tone + NR 双 pass 相乘接入。
  - xmp.rs：`range_mask_xml` 第二组件 `Mask/RangeMask`，相交编码
    `BlendMode=1 + Inverted=true + Value=0`（从用户自己的 LR 边车
    `_DSC9245.xmp`/`_DSC9303.xmp` 解码验证的代数）。
  - gui.rs：选中 mask 面板「范围蒙版」下拉（无/亮度/颜色）；亮度=下限/上限/
    羽化三滑杆（GUI 对称羽化 ↔ recipe 4 数梯形）；颜色=色块 + 🎯 取样
    （`handle_range_pick`：pre-mask develop 的 5×5 均值，与引擎评估像素
    一致）+ 容差滑杆；`range_picking` 入工具互斥。
  - advisor：openai.rs 结构化 schema 加 `range`（anyOf 双变体 + null）+
    prompt 用法指引。
  - 已知近似：(a) 范围权重按"全局显影后、蒙版逐个叠加时"的像素评估——
    多 mask 叠加时后面的 range 看到前面 mask 的输出（LR 是固定参考图；
    全分辨率快照内存不可行，已注释）；(b) 颜色 PointModels 第 4-6 数
    按"取样点坐标+保留位"假设写出，未与真 ACR 对照语义；(c) 真 LR 打开
    效果待用户验收。
- **② 主体/天空 AI 分割 ✅（2026-07-07 凌晨）**：位图 mask 通路 + python
  sidecar 两层全通。
  - **位图通路**：recipe.rs `MaskGeometry::Bitmap { path }`（`kind`-tag 序列化，
    JSON 往返测试）；render.rs `load_mask_bitmap`（每 mask 每次 develop 解码
    一次，绝不进像素循环；缺文件=惰性 + stderr 警告）+ `sample_gray_norm`
    （归一坐标双线性 → 1280 mask 驱动 61MP 导出）+ `mask_weight` 第三臂，
    tone/NR 双 pass 共享；xmp.rs 位图 mask 跳过（经典 ACR XMP 无法表达；
    全位图时不发空壳块——参数 mask 照常写出，§B 式定位取舍）；GUI 列表
    「位图」标签、overlay 徽标（不假装形状）、重画按钮对位图隐藏。
  - **sidecar**（`python/segment.py` + `src/segment.rs` 桥，循 denoise.py
    模式；config `segment_script` / `AUTOSHOP_SEGMENT_SCRIPT`）：
    `--target subject` = rembg U²-Net 显著主体软 alpha（`pip install rembg`，
    模型首跑自动下载 ~/.u2net，176MB；**R27 Batch-4 起显式
    `new_session("u2net")`，防上游默认改成付费模型**）；
    ~~`--target sky` = SegFormer-B0~~ **← R27 Batch-4 起 = OneFormer Swin-L**
    ADE20K 天空类概率（transformers，~14MB 自动下载；sky 类号从模型
    id2label 解析、不硬编码）。缺依赖时 exit 2 + 打印确切 pip 命令。
  - **GUI**：局部调整区「🤖 AI 选主体」「☁ AI 选天空」→ worker 喂
    ORIGINAL 帧预览 → `./out/<stem>.mask-<target>.png`（同 target 重跑
    覆盖同文件）→ 推入 Bitmap mask 并选中，undo 一步回退；软 alpha 即
    天然羽化。
  - **实测（2026-07-07，用户环境 Python313）**：天空 = Lundy 真照片
    天侧均值 254/地侧 0；主体 = 合成主体中心 255/背景 0/覆盖 18.7%
    （与真实面积一致）；纯风景无主体时主体 mask 近空属模型正常行为。
    rembg 需装进 `python` 对应环境（用户机上 `pip`≠`python -m pip`，
    后者才对）。
  - 已知边界：mask 位图不进 XMP（LR 侧丢 AI 选区）；位图 overlay 暂为
    徽标而非半透明叠加显示；分割跑在预览分辨率（对羽化选区足够）。

### B. 像素母版 ↔ 参数配方双轨打通（✅ 已完成 2026-07-06）
- 现状（旧）：fill/heal/clone 输出 ./out 像素母版，仅在 After 显示一次；
  滑杆一动即 redevelop 回配方渲染，母版脱链。
- **已实现**（gui.rs）：`Msg::Retouched`（`RetouchDone` 别名）四条像素路径
  （fill/heal/clone/reimagine）都带回母版路径 → `self.master`；Retouch 面板
  顶部「⤴ 以此母版继续修图」→ `continue_from_master`：一次性 `keep_recipe`
  标志让下一个 `Msg::Opened` **保留当前配方**（母版是同帧中性显影+修补像素，
  滑杆/蒙版/裁剪/拉直 1:1 适用），src_path 重定向到母版 → 后续修图/导出
  都基于它。undo 历史在新 base 上重开；master 随换图清空；打开失败也会
  消费掉 keep_recipe（不泄漏到无关的下一次打开）。
- 边界：XMP 仍只随 RAW 源写出（母版是 PNG，只写 recipe json）——参数轨
  的 Lightroom 出口停在原 RAW 一侧，属定位内取舍。GUI 态逻辑无单测
  （egui app 态），入真机验收列表。

### C. 镜头/几何校正（✅ 暗角 + C2 手动畸变均完成 2026-07-06）
- **暗角补偿 ✅**：`recipe.lens_vignette / lens_vignette_mid`（-100..100 /
  0..100，clamp 齐全）；引擎 `apply_vignette`（render.rs）——**线性光域**
  径向增益 `1 + k·rⁿ`，midpoint 经指数 0.6..3.0 控制作用范围，apply_develop
  第 0 步（tone 前），预览/导出/母版三路径共享；GUI「镜头校正 · Lens」区
  两滑杆；XMP `VignetteAmount`（键名从用户 140 份真边车实证）+
  `VignetteMidpoint`（ACR 文档配对键，用户边车中无非零实例，语义待真 LR
  验证），amount=0 时零键写出（与旧 writer 字节兼容）。单测：中心不动/
  径向单调/负值压暗/高中点收缩作用域；XMP 条件写出。
- **手动畸变校正 ✅（C2，2026-07-06 深夜）**：`recipe.lens_distortion`
  （-100..100，ACR 语义：正修桶形、负修枕形）；引擎（render.rs）
  `distort_norm / undistort_norm / apply_lens_distortion`——半对角线归一的
  单系数径向模型 `r_src = s·r·(1+k(sr)²)`，`k = −amount/100·0.25`（|k|<1/3
  保单调可逆；方向经两条独立推导交叉验证），负 amount 走 Newton 填满缩放
  （无黑角，同拉直的 auto-fill 策略）、正 amount 角部内容自然裁出；逆映射
  Newton 求三次根、被裁内容钳到单调极限落在视野外。管线插入点：三条路径
  （RAW 导出/baked/GUI redevelop）统一 develop 之后、拉直之前。GUI 映射链
  `view_norm_to_orig/orig_norm_to_view/geom_to_view` 全部带 `dist` 项
  （wb 吸管/范围取样/画笔/mask 放置/region/克隆 全调用点接入），镜头面板
  第三滑杆；XMP `LensManualDistortionAmount`（键名从用户 148 份真边车实证，
  仅非零写出）。已知近似：amount→k 增益是我方标定（Adobe 未公开），同数值
  下 LR 的校正强度可能不同——入真机验收单。单测：映射双向 roundtrip
  （4 幅度）/方向性/双符号无黑角/中心不动点/内容径向外移。
- **未做**：per-lens profile 校正（lensfun / 厂商 k1+k2 多项式——手动滑杆
  已覆盖目测校正，按镜头自动化留长期项）；去紫边（需边缘邻近门控，防误伤
  紫色主体）；透视 Upright。
- AI advisor 暂不暴露镜头字段（校正是测量性操作，非审美建议；schema 未加）。

### D. 色彩管理（✅ sRGB ICC + D2 广色域输出均完成 2026-07-06）
- **导出嵌 ICC ✅**：`render_to_file` 三种格式全部显式编码器 + `tag_icc`
  （render.rs，原 tag_srgb 泛化）——JPEG=APP2 ICC_PROFILE 段、PNG=iCCP 块、
  TIFF=tag 34675；profile 用 saucecontrol/Compact-ICC-Profiles
  （**CC0-1.0 公有领域**，assets/ 下入库；下载时验证 acsp 签名 +
  repo license API 实证）。单测逐格式验证 marker 字节存在。image 0.25.10
  三个编码器的 `set_icc_profile` 实现已核对（真存储非 Unsupported）。
- **D2 P3/AdobeRGB 输出 ✅（2026-07-06 深夜）**：`ExportColorSpace`
  {Srgb, DisplayP3, AdobeRgb} 入 `ExportOpts`（默认 Srgb，旧调用零变化）；
  **真 gamut 变换**（render.rs `convert_export_color_space`）——解 sRGB
  TRC → 线性光 3×3 原色变换 → 目标 TRC（P3 同 sRGB 曲线；AdobeRGB 纯
  563/256 gamma）；矩阵**运行时从原色色度推导**（`rgb_to_xyz`/`inv3`，
  不手抄七位小数表），三空间共 D65 白点、无色适应项；白点保持单测端到端
  锁定推导。profile：`DisplayP3-v2-magic.icc`（736 B）+
  `AdobeCompat-v2.icc`（374 B），下载时同样验 acsp+尺寸。GUI 导出面板
  「色彩空间」下拉（sRGB/Display P3/Adobe RGB），入 Prefs（越界回落
  sRGB）。未知扩展名（无法带 tag 的格式）刻意留 sRGB——P3/AdobeRGB 数值
  不带 profile 到处都显示错。单测：白/灰/中性保持、逆矩阵 roundtrip、
  sRGB 红在 P3 内（正 g/b）/在 AdobeRGB 是重缩放纯红（共享红原色）、
  JPEG/TIFF 文件字节含完整目标 profile。
- **未做**：egui 显示端色管（上游限制）；retouch 母版 PNG 的 ICC（工作
  文件，导出时会再过 render_to_file 补 tag）；工作空间本身仍是 sRGB
  （引擎在更宽空间显影是另一级大工程，超出导出选项范畴）。

### E. 1:1 真像素检查（✅ 已完成 2026-07-06）
- 现状（旧）：预览固定 1280px（gui.rs `PREVIEW_EDGE`），「1:1」= 预览像素。
- **已实现**（gui.rs）：标题行 Fit/1:1 旁新增预览分辨率下拉
  （1280 流畅 / 2560 / 4096 检查），入 `Prefs` 持久化（恢复时白名单校验，
  防坏存档造出 1px/100MP 预览）；`open_path` 按选中值缩放工作预览；
  **切换即重解码当前照片且配方保留**（复用批次 B 的 `keep_recipe` 通路），
  busy 时下拉禁用。代价如实标注：2560/4096 下每次滑杆调整变慢
  （develop_preview 逐像素成本 ×4/×10）。
- 未做（大工程，暂缓）：全分辨率 tile 金字塔（真 61MP 1:1 平滑漫游）。

### F. 导出管线（✅ 已完成 2026-07-06）
- 现状（旧）：`render_to_file` 只出全分辨率 16-bit TIFF / q95 JPEG；
  批量只同步配方不出图。
- **已实现**：
  - 引擎（render.rs）：`ExportOpts { long_edge, sharpen, jpeg_quality }` 作
    `render_to_file` 第 5 参（`Option`，None=旧行为，main.rs/serve.rs 7 个
    调用点传 None）；顺序=重采样（Lanczos3，永不放大）→ 输出锐化
    （luma unsharp r=1，在**缩放后**像素上）→ 按质量编码；返回保存后尺寸。
    单测锁定：50 长边出 50×25 且文件实测一致、超源尺寸不放大、q30<q95。
  - GUI（gui.rs）：导出区新增 长边下拉（原尺寸/1600/2048/2560/3840/5120）+
    输出锐化滑杆 + JPEG 质量滑杆（选 JPEG 时显示）；三项进 `Prefs` 持久化
    （Prefs 补 `serde(default)`+手写 Default 对齐 app 默认，旧存档不失效）；
    单张 Export/Download 与批量共用 `export_opts()`。
  - **批量渲染**（gui.rs `start_batch_render`）：gallery 多选 →「🖼 渲染
    选中(N)」——每张读它自己的 `./out/<stem>.recipe.json`（无则中性显影）
    按当前格式+导出选项出 `./out/<stem>.developed.*`；单 worker 顺序跑
    （61MP 全幅并行只会抖内存）；汇总成功/失败走 toast。AI Denoise 明确
    不参与批量（GPU sidecar 每张数分钟）。
- 未做（定位内暂缓）：水印、导出预设、色彩空间选项（后者归 §D）。

### G. 历史/版本（✅ 已完成 2026-07-06）
- 现状（旧）：undo/redo 100 步（内存态，关会话即失）；./out recipe json 单份。
- **已实现**（gui.rs）：版本快照 = `./out/<stem>.v<N>.recipe.json`（编号
  递增，不碰工作用 `<stem>.recipe.json`，库只读不变）；develop 面板
  「版本 · Versions」区——「＋ 存为版本」写下一号快照，列表每行「载入」
  替换当前参数（走 dirty→redevelop，撤销一步回到载入前）；列表缓存于
  `self.versions`，照片打开/存版时 `refresh_versions` 重扫（不逐帧扫
  ./out）；载入时 clamp() 防手改 JSON 越界。
- 未做（内存 undo 持久化到磁盘的完整历史——快照已覆盖"多套参数并存"
  的主需求，全量历史留给需要时再做）。

### UX 批次（用户指定方向 2026-07-07 起：UI 与操作细节）
- **第一批 ✅（2026-07-07）**：
  1. **蒙版覆盖叠加显示**（LR 的 O 叠加）——引擎新增 `render::mask_coverage`
     （与 apply_masks 完全同源：geometry×inversion×amount×range，range 在
     masks-cleared develop 参考上求值，单测锁定）；GUI 选中蒙版即显示红色
     半透明覆盖层，经畸变+拉直同一几何链落到 view，随滑杆/选中/O 键实时
     刷新。**同时关闭了"位图蒙版只有徽标"的 A② 已知边界**——位图/参数/
     范围蒙版统一走真实权重显示。开销如实：叠加开启+选中蒙版时每次滑杆
     调整多跑一次 masks-cleared develop（1280 下无感，4096 下可 O 关掉）。
  2. **削波警告**（LR 的 J）——红=任一通道 ≥254 溢出、蓝=全通道 ≤1 死黑，
     按**显影后导出像素**判定；标题行 ▲ 按钮 + J 键，入 Prefs 持久化。
  3. **Esc 统一退出工具**——裁剪/放置蒙版/WB 吸管/范围取样/图章/画笔/
     框选一键全退（画布与取样点保留可续）。
- **第二批 ✅（2026-07-07）**：
  4. **蒙版行 hover 即预览覆盖**——鼠标悬停蒙版列表任意行，图上即显示
     该蒙版的覆盖（不必点选；移开回落到选中项）。靠第一批的参考缓存，
     hover 切换只重算轻量覆盖图。
  5. **直方图削波三角灯**（LR 同款）——直方图左上/右上三角：暗部/亮部
     极值 bin 有像素时点亮（蓝/红），干净时灰；点击 = 切换 J 叠加，
     与 ▲ 按钮/J 键三入口同一状态。
  6. **批量渲染进度条**——worker 逐张上报 `Msg::BatchProgress`，顶栏
     实时进度条 + 状态行计数（此前只有结束 toast，跑长批像卡死）。
  - 滑杆微调核实为 egui 原生已有（点数值可键入、拖数值即精调），不另做。
- **第三批 ✅（2026-07-07）**：
  7. **叠加参考缓存失效收窄**——cache key 里中和 straighten/distortion/
     crop（develop_preview 不读它们，由调用方在其后应用；lens_vignette
     保留因为它是 develop 阶段）——拖拉直/畸变滑杆不再无谓重建参考。
  8. **工具光标语言补全**——平移=抓手（按住空格=Grab/拖动=Grabbing）、
     画笔/放置蒙版/裁剪=十字线；WB/范围/图章取源原有十字线保留。
  9. **蒙版 ⬆⬇ 排序**——蒙版顺序是渲染语义（顺序叠加，后面的范围蒙版
     看到前面的输出），选中行两键上移/下移，动了即 redevelop。
- **第四批 ✅（2026-07-07）**：
  10. **直方图削波三角按通道显色**（LR 同款）——三角颜色 = 极值 bin 里
      哪些通道有像素的加色混合：单通道即原色、双通道黄/品红/青、三通道
      全溢出为白（一眼区分中性压黑/溢出 vs 偏色），tooltip 列出具体通道。
  11. **蒙版真拖拽排序**——列表行为 egui 原生 dnd drag source（拖起浮影
      跟随光标），悬停行中线上下显示插入线，松手落位；`reorder_move`
      （remove+insert 的索引重映射）被单测对每个 (from, insert) 组合与
      真实 vec 操作逐元素比对锁定；拖拽中 `hovered()` 全局为 false，
      hover 预览自动暂停不churn覆盖层；⬆⬇ 按钮保留作精确路径。
  12. **裁剪柄方向光标**——悬停/拖动角柄显示对角线 Resize 光标
      （TL/BR ↘、TR/BL ↙），框内显示 Move；命中判定与 drag 同一
      `pick_handle`（同 12px 半径），不再全程十字线。
- **第五批 ✅（2026-07-07，真机反馈驱动：①窗口缩放挡按钮 ②蒙版体验
  ③操作手感）**：
  13. **顶栏换行不裁剪**——两行工具栏由 `ui.horizontal` 改
      `horizontal_wrapped`（原实现窗口一窄右侧按钮直接被裁掉、无法触达）。
      egui 换行只对**原子控件分配**生效，嵌套 `add_enabled_ui` 作用域在行尾
      会被压扁而非换行，故禁用门控改为逐控件 `ui.add_enabled`；导出**设置**
      （格式/长边/锐化/色彩空间）不再随无照片禁用（本就是持久化偏好），
      只有 Export/Download/Save XMP **动作**门控。另加
      `with_min_inner_size(980×620)` 兜底。
  14. **蒙版图上直接编辑**（LR 手势，不再"重画"从头拖）——选中蒙版显示
      可拖手柄：线性=zero 端/full 端/中点整体平移；径向=中心平移+四边
      中点单边调整（含最小尺寸保护）；写回经 `view_norm_to_orig` 同一
      几何链落回原始帧；拖动实时 redevelop+覆盖层刷新，松手由
      `commit_if_settled` 收为一步撤销；手柄命中优先于框选，框选拖拽
      进行中则框选优先（否则拖过手柄框会冻结）；手柄 hover/拖动
      Grab/Grabbing 光标；位图蒙版无参数手柄（维持徽标）。
  15. **放大后直接拖拽平移**（LR 手势，"手感别扭"主根源）——zoom>1 时
      主键拖拽即平移（原需空格/中键）；Ctrl+拖拽保留框选；激活工具/
      蒙版手柄/进行中的框选均优先于隐式平移；悬停即显抓手光标。
- 待做候选（未开工）：
  - **InPlace 修饰母版无持久移除通道**（R12 批 68 记录，批 69 修正措辞）：唯一
    能分离健康 heal/clone 母版的路径是**会话内 undo**——修饰→保存→Ctrl+Z 回到
    修饰前步→再保存（Save 见画布无 origin 即清链）——不可发现，且重开照片后
    undo 栈为空、origin 从 pixels.json 恢复，通道随会话消失；web 端完全没有
    （中性保存自批 65 起保留母版并在响应中披露）。需要一个显式、可发现、跨会
    话的"移除修饰"动作（确认 + pixels.json 清理 + 画布回源）。
- 快捷键现状：Ctrl+Z/Y/O/E/S、←/→ 走图、B 对比、O 叠加、J 削波、Esc 退出、
  空格/中键平移、**放大后直接拖拽=平移、Ctrl+拖拽=框选**、滚轮缩放、
  双击 Fit↔1:1、滑杆双击归零/点值键入、蒙版手柄拖拽=改形/移动。

### 明确不追（定位外）
- 图层/混合模式/智能对象、文字/矢量、设计合成——PS 的另一半；
  reimagine/fill + 反推配方已覆盖摄影侧的"创意改图"。

### 建议批次顺序（v0.3.x 起 · 2026-07-06 收官状态）
~~A①（范围蒙版）~~ ✅ → ~~B（双轨打通）~~ ✅ → ~~F（导出管线）~~ ✅ →
~~E（高分预览）~~ ✅ → ~~C（暗角 + C2 手动畸变）~~ ✅ →
~~D（sRGB ICC + D2 广色域输出）~~ ✅ → ~~G（版本）~~ ✅；
剩 A②（AI 分割）——待引擎位图 mask 通路，是差距清单最后一个大项。

## 完成每项后的例行动作

1. `cargo clippy --features gui --all-targets`（零警告）+ `cargo test --lib`
   + release build + GUI 启动烟雾。
2. 密钥扫描（`sk-[A-Za-z0-9]{20,}|OPENAI_API_KEY=|ANTHROPIC_API_KEY=`）后
   提交（结尾 `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`），
   用户说 push 才推、说发布才发 release。
3. 攒够一批（如 ①②③）可提议发 v0.3.0。
