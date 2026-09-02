# ROADMAP — AutoShade 发版台账（逐版已发布内容与实测数字）

> 这是**已发生之事的台账**，不是待办表：每一条要么是已发布的版本与实测数字，
> 要么是带理由的终局裁定（一个测出来的数、一条仪器极限、一次用户拍板）。
> 每项都附 `file:line` 或提交锚点，供新会话不重读全库即可接手。更新于 **2026-09-02**。
>
> **v1.2.3 已发布**（2026-09-02，tag `v1.2.3` → `f3885b2`，release run `33641516217`
> 四工位绿，六资产回下载字节校验）；**v1.2.4 清账批进行中**——用户令 2026-09-02
> 「没有 roadmap，没有『待办』，必须、强制、不计代价也要把所有，所有，遗留内容清完」，
> 本文件顶部的 v1.2.4 条目在发版时填写。
>
> 项目自 v1.1.0 起名为 **skymanbp's AutoShade**（短名 AutoShade），二进制
> `autoshade` / `autoshade-gui`，环境变量前缀 `AUTOSHADE_`。官网 **autoshade.dev**
> （Cloudflare Pages 项目 `autoshade`，别名 `autoshop-d7w.pages.dev`；旧域
> `skymanbp-autoshop.dev` 301 过去），部署走 `scripts/deploy_site.js`——现场用仓库根
> `.secret` 的主令牌铸一个一小时期限的 Pages Write 令牌、`finally` 里删掉，令牌值不
> 打印也不落盘。`site/_headers` 给 `/images/*` 七天 TTL（`max-age=604800`）而 URL 固定，
> 所以图片靠 `?v=<release>` 缓存键翻新（`site/index.html` 现为 `?v=1.2.3`）；边缘 purge
> 权限用户已于 2026-09-02 补授给个人令牌，`scripts/purge_site_cache.js` 里连同
> 「主令牌无法委派 Cache Purge」的实证一起在库。
>
> 历史内容——v0.16.1–v1.0.0 层积的旧横幅、各轮计划、2026-08-18 入库的标签定义
> （`M0`/`M8`/`B2`–`B5`/`SF4`/`M-A`–`M-D`）与更早的「当前状态」条目——逐字存放在
> [docs/ROADMAP-archive.md](ROADMAP-archive.md)（追加式档案，勿重写）。

## 版本台账（逐版已发布内容与实测数字，新在上；均已完成，勿重做）

### v1.2.4 — 清账批（本节由发版时填写）

- **🚢 v1.2.3 已发布 2026-09-02（tag `v1.2.3` → `f3885b2`，release run `33641516217` 四工位绿；六资产回下载 `sha256sum -c checksums.txt` 全 OK 且独立 SHA-256 一致（run 33641516217 四工位绿）；README/官网六行资产表由回下载字节回填；官网 deploy_site.js exit 0，verify_live 剥 CF beacon 后 22/22 逐字节相同；官网部署字节校验；本机 Programs\AutoShade 由回下载安装包静默原地升级：注册表 `{B2C8B506-…}_is1` = 1.2.3、autoshade.exe `a505ed5a…` / autoshade-gui.exe `99b04b51…` 与 checksums.txt 全等、权重 19 文件 3,462,853,028 B 未动、PendingFileRenameOperations 无 AutoShade 项、无 GUI 进程）。** 本版三项根因修复各由 Opus 子代理在独立工作树实现、两轮对抗复审（均 refuted=false）后合入主干（合并提交 e58d796 / 12dc779 / 3a9e2fb）：
  1. **色偏曲线把一片天空按亮度扇成多种色相**（cast 分支 `c5e6988`…`6ca89c8`）——三条独立单调通道曲线能在均值色相只动 0.7° 的情况下把 Cornwall 天空的八分位色相从 1.6° 撑到 33.1°，而三道既有否决（外来色相 45°/5%、区域重着色 75°/5%、比值门）全是逐像素「目的地」问题，看不见这种**关系性**缺陷。新增第四道否决 `hue_fan_weighted`（旋转普查同一人口，24 类 15° 色相桶 × 证据模型亮度桶，量「类内切片均值色相的最大环差」减去修前基线；FAN_SHARE 0.05 / FAN_DEG 15，标定：Cornwall 37.6°、合成 44.6°、雾霾修正 7.8°、峡谷 7.5/5.2、雾→鲜艳 2.7；FAN_DEG=20 端到端实验让 19° 的色偏带着 20.6° 交付扇面出货，故取 15）。**投影**（用户裁定「本版就做收缩投影」）：仅当扇面门是唯一失败门时，沿单参数路径 `C(t)=x+min(1,2t)(L−x)+max(0,2t−1)(C−L)` 收缩（t=1 拟合曲线、0.5 三通道共享形状 L、0 无曲线），12 步二分取**渲染候选**过四门+强度准入+`FIT_QUANT` 增益门且扇面 ≤ FAN_PROJECT_DEG=7.5° 的最大 t；任务书原设计止于 L 的家族在 Cornwall 上**惰性**（共享非线性曲线仍改色相比值，t=0 处普查读出 17.3° 新增扇面，高于 15°），实现者据实测延到恒等——主审裁定采纳。救援只在产出用户配方的那一次 `fit_cast_stage` 调用里跑（混色器 do-no-harm 两分支用实测色偏判定，否则四个无关夹具判决漂移）。Cornwall 全局阶段 0.137→0.030、置信 0.664（v1.2.2 带扇面 0.033/0.646，只拒绝 0.058/0.577），t=0.363，交付天空扇面 10.5°（目标 1.6°，v1.2.2 33.1°）；分区求解 0.137→0.027、置信 0.66、交付扇面 9.6°；15° 备选实测（0.026/0.680/15.3°）记录并否决。高架 `match` 配方逐字节同（像素否决先于投影）；雾霾夹具配方逐字节同。普通准入不再沉默：`FIT_NOTE_CAST_ADMITTED` 三句（头句比值对预算界、外来色相与扇面各有「已测/不可测」两句，扇面带符号），投影写 `FIT_NOTE_CAST_PROJECTED` 族，拒绝句扩写「已试收缩」。两个夹具判决因投影而动并重钉：canyon-warm 从整配方重置落到 0.0387→0.0339/0.406；two-family HSL 残差落到 FIT_QUANT_CLEAN 之下、`hsl` 披露转静（小披露回退——即本条末尾点名的六条之②）。
  2. **写了方向时风格库仍主导**（style 分支 `e1df4ab`…`325f500`）——`render_reference` 只有 Ceiling/Target 两种声音、`produce_recipe` 的 `style_pull` 在 Style 1.0 全量替换，方向在措辞上赢、在算术上输。一处决定 `StyleVoice::choose(style, direction, adherence)`：方向非空且遵循档 Direct/Brief（>0.4）→ **Background**（块头「STYLE BACKGROUND … the DIRECTION LEADS」、四条目标句改为可被方向覆盖的习惯、`style_blend_pull` 返回 None、跳过再验证、披露 `STYLE_BACKGROUND` 点名 Adherence 拨盘 40%）；判官简报同声（Background 下自家编辑的共享外观是「延续性」而非 brief，「不许倒退」改指方向）；web `POST /api/analyze` 加可选 `adherence` 字段+页内滑块（缺省 0.65 不变）；`style-query --adherence`。无方向/空方向/Hint 档的块在 0.30/0.90/0.85 边界逐字节同 v1.2.2（夹具钉住）。两处 `blend_toward` 调用点由源码扫描测试钉守（计数+邻接）。岛屿三张在 v1.2.3 发版构建、全索引（169+94）、`--style 1.0 --strength 0.9` 上重渲：饱和 28 % / 11 % / 30 %（中性 17 %）、亮度 43 % / 58 % / 70 %（中性 47 %），v1.2.2 同索引为 23/11/17 % 与 54/58/55 %；只含成片库那张保留在 SHOWCASE 作对照。
  3. **亮度范围边界预算 0.012 实测并保留**（range 分支 `c3e7f98`…`0e3b224`，裁定 B）——`range_transition_rim` 只在参考侧已平滑（|Δl|≤2.5/255）的穿越上量**渲染后**梯度，场景梯度在分子里先被花掉，按 v1.2.2 的上下文计费会**过紧**（乘数 ≥1、上下文封顶 0.0098<0.012，通过条件塌成「场景+修正≤场景」）而非放松；实测（转写尺、twin 基）：标定对 p90 0.00392→0.00230（max 0.00978→0.01217，n=18297）、高架 0.00874→0.00857（max →0.01407，n=1224），引擎栅格 18651/1214 另标；缝式 z 四行（+10.08/+1.94/+6.99/+8.92）基依赖、传递压缩（30 码→9.9）、估计器噪声 ±0.05 全部上记录。新增测试钉窗口/预算/拒绝计费三不变量，三变异转红。**本条末尾点名的六条之①**：两把尺都看不见色调反转（合成斜坡 1.5/17 @−0.56 EV 反转 2 码、rim_overshoot 读 0.0000；2/17 行仪器几何不可测），补救＝交付传递的符号检验。同批关闭 **v1.2.2 说明书的假话**：`audit_i18n` 在 tag 树上实为 exit 1（边界连续性 kept/dropped、无分区修正、残差帧四句无中文 + 1 孤儿），四句译毕、审计各节归 0、字形子集 871/871 未重生成。
  发版电池（并行四车道）：lib 1332 枚举 = 1320 过 + 12 ignore（按名集差对 `8e631f7` **+24 / −0**）、校准语料车道 1320/0/12、CLI 23、契约 2+2、GUI 160、clippy 默认与 gui 均 0 警告、`check_docs --gates` 27 PASS / 0 FAIL / 1 SKIP（28 claims）、`audit_i18n` 0、字体 871/871、照片名 grep 0。**v1.2.3 收尾时点名的六条自身遗留**（用户令 2026-09-02「没有 roadmap，没有『待办』，必须、强制、不计代价也要把所有，所有，遗留内容清完」——六条随即由 v1.2.4 清账批一次清完，处置与数字写在本文件顶部的 v1.2.4 条）：① 色调反转符号检验（range）；② two-family HSL `hsl` 披露转静 → FIT_QUANT_CLEAN 复审；③ `trf` 逐字复制参数值 → `FIT_NOTE_VETO_DISCLOSED` 的 `kind="luma ranges"` 类英文泄入中文 UI，需参数本地化机制；④ `pipeline.rs` 采纳候选的 `STYLE_DISTILLED` 百分比重算 `style_pull` 而非读守卫绑定；⑤ CE 750 行门：fit.rs / fit_zoned.rs / range.rs / i18n.rs / ARCHITECTURE.md 全部超预算；⑥ 未在语料中的双色温实景（`two_temperature_coast` 只有合成夹具）。

- **🚢 v1.2.2 已发布 2026-09-01（tag `v1.2.2` → `01de443`，release run `33569093474` 四工位绿；四资产回下载 sha256 与 checksums.txt 及独立计算三方一致，README/官网资产表按实测回填；官网部署字节校验；本机 `%LOCALAPPDATA%\Programs\AutoShade` 静默原地升级，exe 与便携包逐字节同、权重 19 文件 3,462,853,028 B 未动）。** 本版解冻 v1.2.1 的「零行为改动」封盘，因为重渲展示图时抓出三类缺陷，全部溯源到最上游根因、一类一修：
  1. **接缝：标量边界预算看不见平滑天空的台阶**（`adf5955`）——每穿越预算 `max(context, 3×slope)` 夹 `[1/255, 0.012]`、按 charged p90 排；坡度取冻结 k=1 候选、两条同侧基线取 min（领口不得买预算）；rim/range 族拒绝计费。高架 r0c0 跨界步 0.0278→0.0042（k 0.460→0.121），交付接缝 +3.15→+0.92 码阶；纹理瓦逐字节同。九手工变异全红。地板 1/255 = 仪器极限；**range 族专用 0.012 是判词点名的第二发现**——v1.2.3 在两条基上把它实测后按裁定 B 原值保留（`c3e7f98`…`0e3b224`，常量在 `src/fit_zoned/range.rs:130`；数字见 v1.2.3 条第 3 项）。
  2. **嵌入预览≠传感器帧（一类三路线）**（`fb3ef85` + `6552c44`）——机身设 4:3 的 α7R IVA 把居中裁剪写成预览（1440×1080 over 9504×6336，NCC 居中 0.987）。`reimagine` 按预览规划尺寸把 3:2 显影压成 4:3 送出（D 0.304→0.139 修后）；CLI `match` 在预览上拟合整帧目标（CROPPED 警告、区/瓦蒙版落错帧；修后走 GUI 自 v0.25.0 的「整帧中性显影+组合标定」路线，康沃尔 0.137→0.027）；`photo_base_knots` 整幅对裁剪做 CDF 配对（`render::camera_frame_of` 先居中裁）。一条规则 `fit::same_frame_plausible_dims`（2%）。
  3. **风格索引写 50 键、读 12 键**（`7420c0a`，v1.2.0 `a31eb2f` 引入的回归）——任一 HSL/分级编辑即整库 unloadable（"exemplar 0 has an unsupported setting key"），`--looks` 的合并分支再把它替换成只含成片库的文件、Style 静默读空。本会话岛屿首轮即如此跑（0 示例+94 成片），重跑前先修。`style::setting_bands` 一张表（recipe 自己的 clamp 带），两具名测试 + 变异 M-S1 双红；用户手册令 v1.2.0/1.2.1 建的索引重建一次。
  同版带走两项排队特性（Opus 子代理 `28bed68`，主审读代码合并）：**`--xmp-dir`**（`xmp_pair` 唯一配对规则：镜像树→平铺→同目录，扩展名按目录列表大小写不敏感、stem 精确；develop 链的写侧同目录站点刻意不动）与**增量索引缓存**（`style-exemplars.json` 帧摘要+源戳；实测同库两次构建 reused 169 / recomputed 0、索引逐字节相同）。展示图全部按三支柱重生成于本版：岛屿四格（**用户 2026-09-01 裁定用只含成片库那轮**：索引 0 示例+94 成片——即缺陷 3 让 v1.2.0/1.2.1 索引落入的状态、渲于 loader 修复前的发版前树，v1.2.2 二进制读同一索引文件得同一状态（`style-query` 实证 0 邻居/94 成片）；Style 轴无可作用（rationale 原话）、每趟一张按方向选出的成片作参考图，方向主导：饱和 17%→34/12/29%、亮度 47%→38/61/65%（面板格测）；judge moody 71（引导修订 64 弃）Accept、golden 84→92 采纳但校验者两次退回没设的颗粒、终词 Revise 未保存、vivid 64→72→84 采纳→73 弃 Accept。对照 169 示例+94 成片、`--style 1.0` 同三方向 23/11/18%——自家编辑习惯当目标、方向只在锚内动，三趟均 Revise；用户据此裁定出 v1.2.3 的「方向给定时自家编辑降为背景」档——v1.2.3 已实现为 `StyleVoice::choose`，图注两版都写）；高架/康沃尔 3×2（D 0.180/0.136 重钉 fit.rs；**康沃尔全局阶段准入的逐通道 cast 曲线过了 75°/5% 重着色门却把天空染偏紫——按拟合原样展示并披露，裁定入 v1.2.3：色相保持的 cast 阶段 + 常规 admission 的 rationale 披露注（当日静默）——v1.2.3 以第四道否决 `hue_fan_weighted`、收缩投影与 `FIT_NOTE_CAST_ADMITTED` 三句一并兑现**）。门：lib 1296 pass + 12 ignored / CLI 23 / GUI 160 / 契约 2+2、集差对 d628c80 +29/−1、clippy 0+0、check_docs 27/0/1、i18n 0、字体 871/871、照片名 grep 0。本版点名的四条其后各自闭合：cast 色相＝v1.2.3 的第四道否决 `hue_fan_weighted` + 收缩投影（`c5e6988`…`6ca89c8`）；range 族 0.012＝v1.2.3 在两条基上实测后按裁定 B 保留（`c3e7f98`…`0e3b224`）；侧车错误泄漏路径＝`9097319` 的 `rationale::error_line`（`src/rationale.rs:909`）已接入全部 `{e}` 汇点，2026-08-31 全类扫零漏网；Style 方向主导档＝v1.2.3 的 `StyleVoice::choose`（`e1df4ab`…`325f500`）。GUI 目检按用户「绝不弹窗」标令永远属用户侧，不是本台账持有的项。

- **🩹 v1.2.1（2026-09-01，用户裁定「破例修代码，发 v1.2.1」——**此裁定显式解除了「1.2.0 是最终版本」对本缺陷的封冻**）**：**① 缺陷**＝每台从改名前版本升级上来的 Windows 机器，桌面应用**每次启动**都打 `warning: could not move your preferences from Autoshop to AutoShade. …the next launch tries again.`——披露里「安全」那半是真的（偏好一个字节没丢、照常从旧文件夹读写），「下次再试」那半是假的：下次会以完全相同的方式失败，**永远**。**② 根因**＝`eframe::storage_dir` 在 Windows 上返回**两级深**的路径（`eframe-0.29.1/src/native/file_storage.rs:53`：`OS::Windows => roaming_appdata()`.map(|p| p.join(app_id).join("data"))`），故 `adopt_prefs_between` 实际要把 `%APPDATA%\Autoshop\data` 改名到 `%APPDATA%\AutoShade\data`，而目标的**父目录** `%APPDATA%\AutoShade` 在从没跑过新名字的机器上不存在，`MoveFileEx` 直接 `ERROR_PATH_NOT_FOUND`。修＝rename 前 `create_dir_all(parent)`，该调用自身失败仍回退。**③ 类扫过，一处真缺陷不是三处**：全仓三个 rename 式收养点里，`store.rs:482/486` 的 `current`/`legacy` 同在 `parent` 下、`serve.rs:2075/2076` 同在 `out_dir` 下，两者的目标父目录**本来就存在**；只有偏好这处因 eframe 多追加一级 `/data` 而两级深。**④ 为什么没被抓到**——测试把两个目录建成**同一个已存在 base 下的兄弟**（`base/Autoshop`／`base/AutoShade`，一级深），这个形状 Windows 根本不会产生；更糟的是它第四臂**明写**「目标父目录缺失应当回退」并断言 `FellBack`，等于**把生产缺陷钉成了正确行为**。这与 v1.2.0 自己修掉的两处同类（cfg 关分支不同步测试、只在打标签时才跑的 CI 步），至此是第三例，共同教训见 [[autoshade-macos-adopt-param]]。**⑤ 新测试驱动生产真形状**（`<roaming>/<key>/data`，且移动前先断言目标父目录不存在），不可救的 `FellBack` 臂改成「父目录位置上摆一个普通文件」使 `create_dir_all` 真失败。**可证伪性双向验过**：变异 M1（去掉修复、留新测试）→ **红**（exit 101）；变异 M2（去掉修复、把测试退回旧的兄弟形状）→ **绿**——第二条才是要害，它第一手证明旧测试永远抓不到这个缺陷。两文件均从字节备份还原。**⑥ 同窗完成官网整理**（用户令「各个部分宽度对齐 / 展示图片太多太乱需要归纳清理」）：宽度根因＝全站没有「一个东西多宽、从哪开始」的单一事实源，正文尺在六个值间各自硬编、编号导轨存在两套几何、UA 的 `figure { margin: 1em 40px }` 只被清掉了上下两边——**`.hero-figure` 与每个 `.image-pair figure` 的 40px 左右外边距一直活着**，于是全页最大的图和六张前后对比图都比同列其他图内缩 40px，且 `.image-pair` 声明的 `gap: 1rem` 实渲成 96px。修法＝补上 `--measure-text`／`--rail`／`--rail-gap` 三个缺失令牌并让每处都走它；`.part-b` 的**内容盒本来就对齐**（padding 公式恒等于 `min(100% - 2rem, --measure)`，勿动），断的是它那条画在通栏盒上的 `border-bottom`，改用 `background-size` 把线收回到 measure，同时给 40rem 断点的 `.section` 加 `:not(.part-b)`（原本按源序压过 `.part-b`，640px 以下悄悄取消了通栏却仍按通栏留白）；另修 `.split-section` 在 58rem 断点用裸 `1fr`（下限＝min-content，而 `.quickstart` 里的 `<pre>` 比视口宽）导致 ≤717px 全文档横向溢出。展示图 **12 → 7**（`<figure>` 24→19），五张降级到 `docs/SHOWCASE.md`（**一个图片文件都没删**），官网此前**零处**链接该画廊，本批首次加上（doc-grid 新卡 + Part A 正文 + 两条 Part B 图注），否则「降级」等于对读者删除；图注总量 5291→约 3100 字符、最长 1655→873。缓存键 `?v=1.2.0` → `?v=1.2.1`（22 处）。

- **🚢 v1.2.0 发版收口（2026-09-01，用户令「全量文档更新+readme更新+发布新 release+网页更新+本机安装版本更新，github 那边也跟进一下项目网站网址」）**：**① macOS 工位连红 13 次的根因已修 `410993e`**——`2f67262` 加了 `adopt_pre_rename_root` 与每分支一条测试（无条件断言 Migrated/KeptBoth/FellBack），`e124d1a`（Mac M1-M3，08-31 08:20）把该函数在 macOS 上经 `ADOPT_PRE_RENAME` 常量整体关闭（成文的数据安全裁定：Mac 上从没有过旧拼法、`Library/Application Support` 默认大小写不敏感、而该函数会 **rename** 它所找到的东西），**却没同步那三条测试**（rule 12 漏同步）；最后一次绿是 `b28efd9`(07:07)。修法按仓库内已有形状统一——GUI 的 `adopt_prefs_between(current, legacy, adopt)` 早就把平台决定**当参数注入**（`gui/main.rs:176`，其调用点传 `ADOPT_PRE_RENAME_PREFS`），故其四条分支测试在 Mac 上照跑；store 侧照做，两个生产调用点传同一常量＝**任何平台零行为变化**。**顺带堵上更深的洞**：拒绝分支此前**在任何平台都没有任何测试执行过**（Mac 是唯一会走它的构建，而 Mac 正是电池死掉的地方），现由 `a_build_that_does_not_adopt_leaves_the_old_folder_strictly_alone` 钉住（陌生人目录逐字节不动）+ 源码级钉两个调用点传常量而非 `true`。**跨平台验证**＝把常量强制 `false` 模拟 macOS，五条收养测试全绿；两条亲手变异（去 `!adopt` 守卫／调用点传 `true`）全红，还原 sha256 逐字节同；真 Mac 回证＝run 33470942203 `test (macos-latest)` **success**。**② 用户裁定未兑现，发版前抓出并补上 `cf05c87`**——ROADMAP 三处独立记录（:207/:208/:1031）「macOS 资产 `.app` 与独立 CLI zip **双发**」，而 M3 是用 `.app` **替换**了 v1.1.0 那个 dist zip（mac-report.md:102 自己标了「这是发布面变更，请主审确认」，用户的答复正是双发），工作流只产一个资产。补 `AutoShade-<ver>-macos-cli.zip`，暂存规则**照抄** `build_app_bundle.sh`（同一套侧车排除：无权重／无字节码／无侧车测试），三条断言镜像 bundle 那三条。**③ macOS 侧车自检从未跑过**——`e124d1a` 加的这一步只在打标签时触发，而它与 v1.2.0 之间没推过标签，首跑即红 `ModuleNotFoundError: numpy`（`segment.py:986`）；工作流注释称其「dependency-free self-test」是**错读**了函数自己的 docstring（「without loading torch or weights」）。修＝给该步一个自带 numpy 的解释器（runner 的系统 python 是 externally managed，为装一个依赖去覆盖别人的包管理器不是修复），注释改成事实；本机同脚本 `--self-test` 亲跑通过。**④ 发版说明按 v1.2.0 义务清单重写**：此前只写披露批，而标签带 **40 个提交**；对 `git log v1.1.0..v1.2.0` 逐条补齐＝改名本身（v1.2.0 才是 AutoShade 版，v1.1.0 之后才合并）、R1 严格更优臂（`ZONE_MIN_ABS_GAIN=0.012`，`fit_zoned.rs:229`）、R2 共有内容参照人口（`CONFIDENT_MATCH=0.5`／`SHARED_POPULATION_MIN_RETENTION`＝证据模型自己的 0.35 地板，`fit.rs:1822/1942`）、对称蒸馏五通道、生成侧色彩量纲（守栏数字回源 `advisor/openai.rs:96-98`，**语料曲线数按源码 17 而非提交信息的 18**）、三处首启目录迁移表、兼容层退休表（`x:xmptk` 两拼法永久）、macOS 资产形状。**另修一条错的升级指引**：原文让用户重建 style index，而短数组零填充、零在习惯地板之下（`mask_habit.rs:250/270`），旧索引照读且不虚报——重建是为**拿到**新的 hue 列，不是为恢复功能；真正断的是反向（v1.2.0 的 11 宽索引进旧 build）。**⑤ ARCHITECTURE 计数重钉** 1268 pass + 12 ignored（旧钉 1263 是披露批之前刷的），增量链补上缺的五条具名测试（`13cebf9` +2、`693917b` +2、`410993e` +1）；README 同组数字同批同步——**是 check_docs 的 battery 断言抓出的漂移**。**⑥ 支柱 1 图注陈旧已修**：检索得分是 `d14 + emb + txt + desc` 四项（`style.rs:1845`），图上写「three rulers」；改生成脚本后重生成（docs/images 与 site/images 是逐字节副本，禁止手改），另四张图重生成后逐字节不变＝确定性回证。**⑦ GitHub 仓库 Website 字段已设** `https://autoshade.dev`（此前为空；旧域 `skymanbp-autoshop.dev` 301 过去，仓库内仅存的三处旧域引用都在 ROADMAP 历史条目里，属当时事实，不改）。**⑧ **v1.2.0 已发布并全链回验（run 33471768908 四工位全绿，tag 指 161a262）**：七资产上架，六个文件的 SHA-256 由回下载字节独立算出并与 checksums.txt 逐一相符；两个 macOS 资产在 publish 前即已开箱验过内容（CLI zip 34 项＝CLI+侧车+assets、无权重/无字节码/无侧车测试、不含 GUI；.app 38 项＝双二进制+icns+签名，plist 四键正确）。README/官网资产表按实测回填（脚本只读下载件并与 checksums.txt 交叉校验，任一不符即拒写）。**官网部署后发现并根治一类缓存缺陷**：apex 仍以 cf-cache-status: HIT / Age 80753 返回旧的支柱 1 图（11,518 B「three rulers」），而不在该缓存后的 pages.dev 别名已是新的 11,312 B——根因是 _headers 给 /images/* 七天 TTL 却挂在可变且名字固定的 URL 上；purge 走不通（探针证实 .secret 主令牌无法委派 Cache Purge：新令牌 active 且能 GET /zones/<id>，purge_cache 仍 401），故改用版本化缓存键 ?v=1.2.0（27 处引用全改，文件名不动故 docs/images 与 site/images 仍逐字节同），重部署后线上八文件逐字节全对（HTML 仅多 CF 注入的 beacon）。scripts/purge_site_cache.js 连同其失效原因一并提交而非悄悄丢弃。**本机安装已按用户裁定迁到 `%LOCALAPPDATA%\Programs\AutoShade`**：先把 881,203,461 B / 8 文件的 OneFormer 权重整体移出（同卷 move）→ 静默卸载(exit 0，注册表项与 PATH 条目均清，残留仅一个运行时生成的 .pyc)→ 以发布件安装(SHA 9a36788d… 与资产表相符)→ 权重搬回并按数量与字节回验一致；安装出来的两个 exe 与发布的独立 exe **逐字节相同**，PATH/开始菜单/卸载项全部指向新目录，`%LOCALAPPDATA%\Programs\Autoshop` 已不存在，全程未启动 GUI。GitHub 仓库 Website 字段设为 https://autoshade.dev。**当时的两条未完成现均已了结**：清账 E5（展示图重渲）由 v1.2.2 与 v1.2.3 各重渲一轮兑现（岛屿四格、高架/康沃尔 3×2 与三支柱面板，数字见那两条）；R3 本机终检只剩把工作目录 `D:/Projects/Autoshop` 改名这一步——那是用户本机的一次目录操作（桌面改名脚本已备好，HF 缓存 junction 与 `~/.codex` trust 随之重建），与仓库内容无关，故本台账不再持有该项。**

- **🔍 用户追问「局部统计量那么多，为什么还救不回来」→ 主审第一方诊断，揪出边界门归因缺陷（2026-08-31 诊断；v1.2.0 的步 #9 已根修，见条末）**：查岛屿对实际配方与理由文本，局部生产者**全部跑了、且各自具名弃权**——①空间瓦片**确实附着** 3 块（底行 r3c3/r3c0/r3c2，EV 0.084–0.160，r3c0 通道增益 `[1.0505,0.9865,0.9427]`＝正确方向的加暖），但按设计受瓦片深度帽与逐 bin 帽约束、幅度小；②双边网格局部场 `realized 0.000`（bin 3/4/5 实测离散 21.80/24.27/20.68 per 255 > 自身上限 15/255 ⇒ 跳过）；③亮度范围因零证据段整体扣留；④色带 Orange/Yellow 单侧不可测被否决，且逐带色彩移动因未使帧更接近目标而**整体退还**；⑤**语义天/地两区拟合成功且更优**（地面区残差 0.078→0.054、质量门纹理比 0.963/1.004、裁切 0%→0%），**却被成对边界连续门丢弃**。**根因（新测）**：该门用 `boundary_rim` 读**单张渲染的绝对亮边**（`fit_zoned.rs:512` 逐扫描线取「过渡带天空侧 luma 最大值 − 已定天空内部中位数」，**无参照图**），而 `shrink_zone_corrections` 的 `k=0` 只把**区**掩码归零（`first_zone = masks.len() − accepted.len()`，区在尾部；拒绝时亦只 `truncate` 掉区），全局阶段与三块瓦片仍在渲染里——**故披露里那句「即使 k=0 仍剩 0.058（预算 0.012）」按门自己的构造就不可能由被它否掉的区修正造成**，否决被记到了一个可证未参与的生产者头上。**为何一直没暴露**：该门四条夹具（`boundary_fixture_pixels`/`soft_zone_pair`）全是**平面合成帧**（天空内部恒 0.20、地面恒 0.40，亮边人工植入；源图 `from_pixel([115;3])` 纯灰），平面帧上绝对读数恰等于增量读数，两种语义重合。**同文件已有增量形式可借**：`boundary_step`（`fit_zoned.rs:685`）算的正是 `rendered_step − reference_step`，供无过渡带的位图蒙版用。**量级实证**：底三分 R/B_lin neutral 0.8126 → 反推后 0.6937（−15%），目标要的是 0.9759（+20%）——全局走反了 15%，而瓦片最大只在部分底行给出约 +11% 的 R/B，**受限的局部精修按架构就无法反转一个错的全局答案**（这是刻意的：局部编辑不得变成第二个全局阶段）。**处置与闭合**：并入步 #9 统计量批一并修，**v1.2.0 的 `9358f2d`（合并 `6c618a7`）已根修**——rim 改为把参考亮度经 `M = linear(rendered)/linear(reference)` 输运后再作差、并按幅值排序（用户裁定），于是 k=0 恰读 0.000，惰性修正走两道门既有的拒绝路径、不再占排除额度；同批把 `ZONE_BOUNDARY_RIM_MAX` 与 `ZONE_BOUNDARY_STEP_MAX` 拆成两个常量（都是 0.012，`src/fit_zoned.rs:165` 与 `:186`），因为一个常量喂两把尺时任一次重标定都会悄悄改动另一把（当时三块瓦片只剩 1.7–3.3% 余量）。本条诊断的另一半——白平衡三条独立逐通道中位数取自不同子人口——同批改为在**一个**人口上解逐像素对数比的加权中位数（岛屿对整帧中位数比 0.8293 对线性均值比 0.9991）。验收要求的「带真实结构的夹具」由 v1.2.2 的 `adf5955` 补齐：硬栅格族改按穿越计费（`max(场景自身台阶, 3×修正同侧坡度)` 夹在 `[BOUNDARY_STEP_FLOOR, ZONE_BOUNDARY_STEP_MAX]`，`src/fit_zoned.rs:186/197`），高架帧 r0c0 跨界步 0.0278→0.0042、交付接缝 +3.15→+0.92 码阶。

### 勘误（对已发布说明的更正；已发布的说明文件本身不追改）

- **v1.1.0 的库 API 断裂从未写进它的发版说明（2026-09-02 补录）**：`0399c88`
  （faithfulness scaffold）把 `generative::reimagine` 的签名从
  `(cfg, raw_path, prompt, fidelity, quality, out) -> Result<()>`（v1.0.0：
  `git show v1.0.0:src/generative.rs`，第 127 行）改成多一个 `retry_on_divergence: bool`
  参数、返回 `Result<ReimagineReport>`（现树 `src/generative.rs:79`(结构体)/`:199`(函数)）。
  仓库内的调用点同批改齐，但 `docs/RELEASE_NOTES_v1.1.0.md` 通篇没有这一句
  （`grep -c ReimagineReport` = 0），把 `autoshade` 当**库**用的外部调用者升到 v1.1.0
  会编译不过。此处补记为勘误：这是 v1.1.0 的第三处硬前向断裂（另两处是 v1.0.0 的
  `mask_warp_center` 与 `linear_handle_warp`，见下面的 v1.0.0 义务清单）。

## 终局裁定与外部事实（非待办）

> 本节收的是**不会再变的决定**和**关于世界的事实**。它们不是开项：每条都写明谁定的、
> 依据是什么。

- **macOS 公证**：用户 2026-09-02 决定**不买** Apple Developer ID，**ad-hoc 签名即终态**。
  后果已如实写在 README（`README.md:103` 与 `:526`：首次打开会被 Gatekeeper 拒，需右键
  「打开」一次；`:543` 另说明不得往 bundle 内写文件，否则签名失效）。
- **五张渲染残差的 Lightroom 导出**：用户 2026-09-02 决定按清单自己从 Lightroom 导出；
  测量在这些文件到位那天做。素材在用户侧，不是仓库项。
- **macOS 交互试用**：至今**没有人**报告过 Mac 上的交互问题（.app 自 v1.2.0 才有）。
  这是关于世界的事实，不是任务。
- **用户提的三条外部工单全闭**：issue #2（Windows Defender 误报）经微软 2026-08-28
  判定 no malware，由用户回帖关闭；discussion #4 已答复并于 2026-09-02 关闭；
  discussion #3（Mac 版）以 v1.2.0 的发版评论答复。
- **旧存储目录收养永久保留**：从 ≤ v1.1 升上来的用户必须还能找到自己的 develop 库，
  故 `store::adopt_pre_rename_root`（`src/store.rs:477`）不退休；macOS 因从没有过旧拼法
  且 `Library/Application Support` 默认大小写不敏感，按 `ADOPT_PRE_RENAME`
  （`src/store.rs:434`）整体关闭。改名兼容层里永久的只有它和 `x:xmptk` 两旧拼法（旧边车
  不自我升级）——`AUTOSHOP_*` 环境别名门（`src/config.rs:890`）、`autoshop.local.json`
  回退与改名前标记的退休归 v1.2.4 清账批，处置见顶部该节。
- **开发期绝不启动 GUI 可执行文件**（用户标令）：连 `autoshade-gui.exe --version` 也不行
  ——它会直接开窗。GUI 车道是测试二进制 `cargo test --features gui --bin autoshade-gui`；
  真机目检由用户自己跑。
- **git 提交不加任何 `Co-Authored-By` 尾行**（用户令 2026-08-31），历史不改写。
- **照片文件名不进文档**（用户令 2026-08-24）：README/官网/文档只用场景名 + 相机标注；
  源码与夹具里的真名已在 `0969d9e` 全部别名化为稳定 P 码。
- **清账 41 项台账**：v1.2.0 之前收口 15 项（见归档里 2026-08-31 各条），其余 26 项
  （A 组 9 / B 组 1 / D 组 8 / E 组 5）由 v1.2.4 清账批一次清完；其中转观察的五条
  （A4/A9/A11/D9/D11）本就是带理由的终局裁定，不是开项。

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
  `Normal`**（rawler 源码里那行注释的原文是 `//cam.orientation, // TODO fixme`——上游 crate 自己的字样，不是本仓的项），DNG/QTK 之外全部解码器如此——
  所以 v0.29.x 以前竖拍 ARW 在显示/显影/导出全链都是横的。**五个**消费点
  （render.rs 渲染钩、decode.rs 的 Meta 尺寸 + 预览转正、`camera_rendition`、
  `frame_size`（v0.32.0 起）、`pipeline::migrate_recipe_coord_frame` 按路径
  孪生——2026-08-20 深检由三勘正为五，ARCHITECTURE 同句早已改）
  均改读该访问器；缺 tag 回 `Normal`，rawler 自己的 `from_tiff` 回 `Unknown`，
  二者在像素/坐标/尺寸三条链上均为 no-op（断言在
  `unknown_and_normal_are_the_same_no_op`）。GUI 缩略图磁盘缓存盐现行 **v4**
  （`src/bin/gui/util.rs:1609`：v2＝烘焙图应用 EXIF 方向、v3＝RAW 同款、v4＝R27 起把
  摄影师自己的 `quarter_turns` 也编进键），否则旧缓存继续端出歪图。**这与风格索引版本
  不是一回事**：`style::CURRENT_INDEX_VERSION` 现为 **5**、可读 `[4, 5]`
  （`src/style.rs:39/47`）。
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
  先例，已写进发版说明）。迁移**只挂在读文件的载入点**（GUI 开图 /
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
  `manual_vignette_lut` 的诚实口径）。**两处与全局链的残差如实记录**（局部
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
- **R30 步 8 — 自动亮度范围分区（`be85702`，v1.1.0 起发布；模块自 `6165097` 起独立在
  `src/fit_zoned/range.rs`）**：保留「全局 fit 优先」，并把
  本地生产者定为互斥：天空分割成功时沿用语义天空/地面位图路径且不推导范围；
  分割被禁用或不可用时，纯 Rust 路径在既有 17-bin 证据上按 `0.03` 有符号
  残差组成连续段，最多四段，按当前渲染从暗到亮各拟合一次。相邻 ramp 为
  `1/17..=2/17`，估计权重总和归一到 `≤1`，各族边界门的预算都是 0.012，但自
  v1.2.0 `9358f2d` 起是三个各自命名的常量而不是一个：羽化过渡带走
  `ZONE_BOUNDARY_RIM_MAX`（`src/fit_zoned.rs:165`）、硬 0/255 栅格（空间瓦片与自由蒙版）
  走 `ZONE_BOUNDARY_STEP_MAX`（`:186`，v1.2.2 `adf5955` 起按穿越计费）、亮度范围走
  `range::RANGE_BOUNDARY_RIM_MAX`（`src/fit_zoned/range.rs:130`，v1.2.3 在两条基上实测后
  按裁定 B 保留）；收缩仍是保方向二分。每段沿用 `attach_one_zone` 的稳健配对、证据、占比、
  correspondence、局部质量与参数化帧门；语义区保留 `0.02` 漂移保险，亮度范围
  使用 `RANGE_FRAME_REGRESSION_TOL = 0.0`，合成证据加权帧变差即逐段放弃；
  零可接受段保持全局配方逐字节不变。
  持久化使用 `MaskRole::Custom`、确定性英文名与全画面 LINEAR 哨兵交集，故
  recipe schema/XMP grammar 均不升级；该批不产出 color range，此后也没有（终局裁定见
  「终局裁定与外部事实」；作为蒙版类型的 `RangeMask::Color` 本就在）。合并、逐段
  放弃、收缩 k 与零差分仍失败均走 typed rationale，GUI 卡显示原生亮度范围
  及四个有序边界。
  **一条已定的观察结论（主审裁定）**：Full 模式范围带的色彩增益无独立帽——带自身
  D<0.65 走 Full 与语义区同 D 同规则并非不对称，且修正后的秩配对派生在旗舰
  对上不再提出大色彩方案（修正前位置配对曾提出 [1.30,0.87,0.75]、被帧门弃）。
  裁定＝**不加**带级分歧比例帽：一个「过帧门却发明色彩」的实例都没有，为一个没出现过
  的形状加帽，先误伤的是已经测到的正例。
- **R30 步 9/10 — layered spatial fit, gated mask refinement, free masks**
  (`67084b2`, `49a796b`, `d21304a`, `662b688` — all shipped in v1.1.0): the order is
  `global -> (semantic OR luminance ranges) -> quadtree tiles -> free masks`
  (`fit_zoned::run_local_sequencer`, with a local-field stop verdict between stages and
  `FREE_MASK_MAX_ATTACHMENTS = 2`); the first two
  local producers remain exclusive. Spatial derivation intersects normalized
  rectangles with evidence frozen from the original pair, traverses best-first,
  stops at depth 2 (4x4) and four accepted leaves, and re-derives after every
  attachment. Both evidence shares must be at least 3%, original `D < 0.65`, the
  weighted 95% interval must exclude zero, and child/parent residuals must differ
  by at least `2/255`. Tiles share the robust estimator and the hard-raster step gate
  (`ZONE_BOUNDARY_STEP_MAX`, also 0.012, contextual per crossing since v1.2.2) but
  use zero composed-frame regression tolerance. They persist as existing Custom
  bitmap adjustments at a 2048 long-edge cap: recipe JSON is lossless and
  classic XMP emits the named bitmap loss, with no gradient approximation.
  Dependency-free guided refinement (radius 8, epsilon `(4/255)^2`) runs only
  before semantic/tile fitting, restores every non-collar pixel, caps coverage
  drift at `0.002`, and abstains when Sobel guide-edge alignment decreases; it
  never touches luminance ranges (it does run on free masks,
  `src/fit_zoned/freemask/attach.rs:109`). No recipe-era change and no new toggle.
  **Multi-class semantic production is no longer out of scope**: step 12 (`a2173c9`,
  shipped in v1.1.0) added `match --zoned --regions 2..4` and the four-region checkbox
  in the GUI, one OneFormer inference per frame through `segment::segment_multiclass_file`
  (`src/segment.rs:147`), pinned by `multi_class_planes_are_normalised_and_ordered`
  (`src/segment.rs:1370`).
- 源照片库只读（`pipeline::guard_readonly`）；输出走 `config::delivery_root()`
  （R24 M8 起为一等设置：settings `out_dir` > `AUTOSHADE_OUT_DIR` > 默认
  `./out`；guard 把配置根与字面 `./out` 都算输出区——见 ARCHITECTURE §4.10。
  原文「输出一律 ./out」滞后于 R24，2026-08-20 修正；env 名 2026-09-02 订正为改名后的
  `AUTOSHADE_OUT_DIR`——见 `src/config.rs:557` 与 `:852`，旧拼法 `AUTOSHOP_OUT_DIR` 至
  v1.2.3 仍由 `canonical_env_name`（`src/config.rs:895`）归一化接住）。

## 完成每项后的例行动作

1. **电池四道**（互不依赖即并行；各用各的 `CARGO_TARGET_DIR`，一律 `--offline --release`）：
   `cargo clippy --offline --release --all-targets` 零警告；lib 道
   `cargo test --offline --release --lib` 跑两遍——一遍不设、一遍设
   `AUTOSHADE_FIT_CALIBRATION_DIR`；全靶道 `cargo test --offline --release`（CLI 集成 +
   契约）；GUI 道 `cargo test --offline --release --features gui --bin autoshade-gui`。
   **GUI 道是测试二进制，不是启动 GUI**——用户标令：开发期绝不运行 `autoshade-gui.exe`，
   连 `--version` 也不行（它会直接开窗）；真机目检由用户自己跑。
2. **文档与资源门**：`python scripts/check_docs.py --gates` 0 FAIL（一遍裸跑、一遍设
   `AUTOSHADE_CENSUS_ROOT`）、`python scripts/audit_i18n.py` 0/0/0、
   `python scripts/subset_gui_fonts.py --check` OK；测试名集差对基线按名逐条交代。
3. **提交前双扫**：密钥（`sk-[A-Za-z0-9]{20,}|OPENAI_API_KEY=|ANTHROPIC_API_KEY=`）与照片名
   （`_?DSC` + 4–5 位数字、`A7R0` + 数字、微信导出名、`IMG_` + 4 位数字这四种机身/导出
   stem——正则本身刻意不写全，否则本文件自己会被那道扫描命中）都必须 0 命中；提交信息**不加**任何
   `Co-Authored-By` 尾行（用户令 2026-08-31）；用户说 push 才推、说发布才发 release。
4. **禁止 `cargo fmt`**：仓库没有 rustfmt.toml，手写风格与 rustfmt 默认差约 1.5 万行，且
   `cargo fmt -- <文件>` 不是文件过滤器（2026-08-12 误伤 43 文件已还原）——格式靠手写对齐
   周围代码。
5. When a release-sized batch accumulates, propose the next SemVer version appropriate to its compatibility boundary; never hard-code an already-released version here.

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

## v1.2.0 发版义务清单（滚动，发版步汇编终稿）

- **C2 存储名迁移（2026-08-31，用户裁定「直接改，不接受旧名字」）——首启一次性数据迁移**：release notes 必须说明
  v1.2.0 首次启动把 eframe 偏好目录 `%APPDATA%\Autoshop`（窗口几何/最近库/主题等）整体改名 `%APPDATA%\AutoShade`、
  导出登记簿 `.autoshop-export-registry` 在首次使用时整体改名 `.autoshade-export-registry`（全部 claim 逐字节随迁，
  交付文件名后缀不变）；两处失败臂都回退旧名继续工作、下次重试；macOS 偏好收养按店教义禁用（从未有旧名）。
- **macOS 资产形状（用户裁定 2026-08-31）**：`.app` 与独立 CLI zip 双发；资产表随之改。
- **改名兼容层退休表**：`LEGACY_ENV_PREFIX` / `LEGACY_SETTINGS_FILE` / `MARK_PRE_RENAME` 三个旧别名按 v1.2.0 义务保留一版供
  升级用户过渡（r2-checklist §6），其退休归 v1.2.4 清账批（顶部该节）；`x:xmptk` 两旧拼法**永久**保留——旧边车不自我
  升级；旧存储目录收养同样**永久**保留，理由见「终局裁定与外部事实」。

## v1.1 发版义务清单（已兑现：v1.1.0 的发版说明与 README 按本清单写就）

> 本节是 v1.1.0 发版当时的义务汇编，逐条已落进 `docs/RELEASE_NOTES_v1.1.0.md`；保留原文
> 作为「当时承诺了什么」的证据，被 v1.2.x 改动或订正的条目在该条内直接注明。下文引用的
> 环境变量是 v1.1.0 当时的拼法（`AUTOSHOP_*`）——v1.2.0 起前缀为 `AUTOSHADE_`，旧拼法至
> v1.2.3 仍由 `config::canonical_env_name` 归一化接住；CI 的 GUI 步当时叫
> `--bin autoshop-gui`，现为 `--bin autoshade-gui`。

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

- **空间瓦片的边界连续性门（v1.0.x 确诊空转，v1.1 已根修 `0ecc2e0`——硬族改跨边界台阶差分中的差分、软族保原 rim 尺（typed 双尺，实测两尺 3/10 翻判不可互换），零可测过渡=拒绝非通过；石桥接缝实测见 ROADMAP-archive.md 的步 10-B1/B2 条）**。原确诊记录如下，保留为证据：
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
  **根修已做完，本条自此是「已修」，上面保留的是原确诊记录**：v1.1 的 `0ecc2e0` 把硬栅格族
  换成跨边界台阶（差分中的差分）尺、软族保留原 rim 尺（typed 双尺，实测两尺 3/10 翻判，
  不可互换）；v1.2.0 的 `9358f2d` 把软族的 rim 也改成输运后作差，并把两族预算拆成
  `ZONE_BOUNDARY_RIM_MAX` 与 `ZONE_BOUNDARY_STEP_MAX` 两个常量；v1.2.2 的 `adf5955` 再把硬族
  改成按穿越计费。验收按当时定的做法用 `scripts/rim_overshoot.py`（无蒙版尺）实测：石桥接缝
  消失，高架 r0c0 跨界步 0.0278→0.0042、交付接缝 +3.15→+0.92 码阶。

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
  范围，无可接受段时保留全局结果。该批（v1.1）没有新 CLI/GUI 范围开关，也不产出
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
  refined. — **勘误（2026-09-02）**：原文末句「Multi-class semantic masks remain explicitly
  out of scope」在 v1.1.0 当天就已不成立——同版的步 12（`a2173c9`）正是多类语义分区
  （`match --zoned --regions 2..4`）。现行事实见「关键架构事实」。

### v1.1 收口裁定（2026-08-30，主审记录在案）

- **色彩范围区**：**终局＝不做**。逐带 HSL（4a，`ab01520`）已覆盖「按色带选人口改色彩」的主面；独立 color-range **生产者**是新特性而非缺陷，v1.2.0–v1.2.3 全线都没为它开渲染面，理由与当时一字不改。作为**蒙版类型**的 `RangeMask::Color` 本就在 recipe/XMP/引擎里（`src/recipe.rs:2007`），需要时用户手工可用。
- **CE 冗余清理批 + 超预算文件拆分**：冗余清理**已做完**（2026-08-31 的 B2a/B2b/B2c/D1/A12 五条 `ce8a804`，加 B1a 侧车样板 `1d3e1fc`，见归档）。超预算文件拆分 v1.2.0–v1.2.3 未做，现状实测（ec7f01e，`wc -l`）：store.rs 9368 / fit.rs 14060 / style.rs 9202 / fit_zoned.rs 7540 / pipeline.rs 7275 / ARCHITECTURE.md 3387 / config.rs 2873 / i18n.rs 2287 / range.rs 2057 / check_docs.py 983 / calibrate_style_retrieval.py 764——它就是 v1.2.3 条末点名的六条之⑤，处置见顶部 v1.2.4 条。
- **4a′ 合成 Full 钉 + `UNREPRESENTED_HUE_DEG` 带质心路测试**：带质心路**已有测试**——夹具 `two_family_hsl_pair` 与 `solving_the_bands_takes_the_colour_shape_out_of_the_residual`（`src/fit.rs:13726` / `:13962`；阈值常量 `src/fit.rs:5419` = 20.0）。v1.2.3 的收缩投影使该对残差落到 `FIT_QUANT_CLEAN` 之下、`hsl` 披露随之转静，即 v1.2.3 条末六条之②。
- **逐方向 β**（OLS 斜率 0.247–2.000）：**终局裁定＝不拟合**——那会是 12 个文本上的自由参数，语料撑不起；出厂单一 β 保持不动。
- **`style-query` 未打印 `txt_hub_corrected` 披露位**：**已修**——2026-08-31 清账 A8（`ed8d4e6`）给 terms 行补了三态披露（`,hub=<均值>` 命名被减去的值／裸 `,hub` 表示修正在场但该对无余弦／前词表索引不打印，逐字节同修正问世前），CLI 测试 22→23。
- 这五条各自了结：**描述缓存跨库 GC**＝2026-08-31 分诊判增长主张不成立（单文件在 store_root、有 20,000 条容量上限，即回收规则），真症状只是跨库交替构建的保留抖动，不改码；**侧车家族样板重构**＝`1d3e1fc` 折进 `python/_sidecar.py`；**AdherenceTier 命名**＝清账 A12（`ce8a804`）给出单一拼写 `as_str`/`prompt_name`；**staged frames 累积**＝不成立，`StagedFrame` 的 `Drop` 逐个删掉 png/json（`src/style.rs:1195-1200`），且名字带 pid+序号+tag 不互撞，只有进程被杀才留残件；**四重名 stem**＝2026-08-31 实测 169 例索引恰 4 个八位机身计数名各由 2 张不同目录的照片承担，每个键控面已按路径消歧且有测试，只有披露文本可能同名，不改码。
- **`W_LOOK` 不可测**：**终局裁定＝出厂 1.0 不动**（谐波在校准尺外，无法上尺）。**2026-08-31 勘误**：旧记的「归一化」不成立——有方向词时 `txt`/`desc` 也在 look 之间排序，故 W_LOOK 的 scale 是真比值、能改顺序；出厂 1.0 落在实测稳定带内（0→2x 序不变，4x 首次重排），两档各由具名测试钉住。
- **xmp 普查钉值刷新**：本收口已完成（记录在 ROADMAP-archive.md 的改名 R1 条）。
