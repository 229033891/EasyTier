# EasyTier锛氬熀浜?GitHub 浜х墿鐨勫畨瑁呬笌鍗囩骇鏂规

## Status

- Status: **Roadmap**锛坄releases/v*` 鑷姩 Publish 宸茶惤鍦帮紱瀹㈡埛绔唴鏇存柊浠嶅睘鍚庣画锛?- 鏈€杩戝闃咃細2026-10-03
- 鐩爣锛氬悗缁?*浠?GitHub Actions / Release / GHCR 涓哄敮涓€鍙戠増鏉ユ簮**锛岀粺涓€瀹夎涓庡崌绾ц矾寰?- 绱㈠紩锛歔`../README.md`](../README.md)
- 閰嶅锛?  - [`../ops/deploy-install.md`](../ops/deploy-install.md)锛圠inux 浜や簰鑴氭湰鐢ㄦ硶锛?  - [`../ops/web-upgrade.md`](../ops/web-upgrade.md)锛堟帶鍒跺彴淇濆簱鍗囩骇锛?  - `docker-compose.yml`锛圖ocker 閮ㄧ讲绀轰緥锛?  - `script/easytier-install.sh` / `script/install.ps1`
  - 璁ㄨ鏉愭枡锛歔`discussion-proposal-2026-10.md`](./discussion-proposal-2026-10.md)銆乕`market-comparison-2026-10.md`](./market-comparison-2026-10.md)

---

## 1. 浜х墿鐭╅樀锛圙itHub 鐢熸垚锛?
| 娓犻亾 | Workflow | 浜х墿 | 鐢ㄩ€?|
|------|----------|------|------|
| Linux 鏃犲ご鍖?| `ET Linux` | Artifact `ET-linux-x86_64`锛歚ET-core` / `ET-cli` / `ET-web-embed` | VPS / NAS 浜岃繘鍒堕儴缃?|
| Windows 鏃犲ご鍖?| `ET Windows` | Artifact `ET-windows-x86_64`锛氫笁涓?exe + `wintun`/`Packet`/`WinDivert` | Windows 鏈嶅姟鑺傜偣 / CLI |
| Windows GUI | `ET Windows` | Artifact `ET-gui-windows-x86_64`锛歂SIS 瀹夎鍖?| 妗岄潰瀹㈡埛绔?|
| Android | `ET Android` | APK | 鎵嬫満绔?|
| Docker 闀滃儚 | `ET Docker` | `ghcr.io/<owner>/et:<tag>`锛堝彲閫?Docker Hub锛?| 瀹瑰櫒閮ㄧ讲 |
| 姝ｅ紡 Release | `ET Release` | GitHub Release 闄勪欢 zip | **瀵瑰瀹夎鑴氭湰榛樿涓嬭浇婧?* |

绾﹀畾锛?
1. **鏃ュ父楠岃瘉**锛歚workflow_dispatch` 鎵?Artifact锛汥ocker 鐢ㄥ搴?`ET Linux` 鐨?`run_id` 鎵嬪姩瑙﹀彂銆?2. **瀵瑰瀹夎**锛氳蛋 **Release 闄勪欢**锛坄script/easytier-install.sh` / `install.ps1` 榛樿璇?`229033891/EasyTier` 鐨?latest/tag锛夈€?3. **Docker**锛氫紭鍏?`ghcr.io/229033891/et:<tag>`锛涘嬁鍐嶄緷璧栨棫闀滃儚 `easytier/easytier`锛圗NTRYPOINT / 浜岃繘鍒跺悕涓嶅悓锛夈€?
---

## 2. 鎺ㄨ崘鍙戠増娴佹按绾?
```text
dev  鈹€鈹€(鎵嬪姩)鈹€鈹€鈻? ET Linux / Windows / Android
       鈹?                     鈹?       鈹?                     鈹斺攢鈻?ET Docker锛堝～ Linux run_id锛屾墦 tag 濡?dev锛?       鈹?releases/vX.Y.Z 鈹€鈹€(push)鈹€鈹€鈻?ET Linux + Windows + Android锛堝苟琛?Artifact锛?       鈹?       鈹斺攢鈻?**ET Release 鑷姩**锛堜笁绔悓 commit 鍧?success锛?             鈹斺攢鈻?鐩存帴 **Publish** Release锛坱ag = 鍒嗘敮鍚嶅幓鎺?releases/锛屽 v2.7.1锛?             鈹斺攢鈻?鑻ュ悓 SHA 涓?OpenWrt 涔熸垚鍔燂紝涓€骞堕檮涓?```

**瀹夎鑴氭湰鍙宸?Publish 鐨?Release**锛堟湁鐗堟湰鍙枫€佸彲鍥炴粴銆佸浗鍐呴暅鍍忓彲缂撳瓨锛夈€侫rtifact 浠呬緵寮€鍙戣嚜娴嬩笌鎵?Docker銆?
### 2.1 宸茶惤鍦帮細`releases/v*` 鑷姩 Publish

Workflow锛歚.github/workflows/release.yml`锛坄ET Release`锛?
**鑷姩璺緞**锛坄workflow_run` 瀹氫箟椤诲湪榛樿鍒嗘敮 `main`锛夛細

1. 鎺ㄩ€?/ 鏇存柊 `releases/vX.Y.Z` 鈫?鑷姩璺?ET Linux / Windows / Android  
2. 浠讳竴绔垚鍔熺粨鏉?鈫?鍞ら啋 `ET Release`  
3. 璇?commit 涓?*涓夌閮藉凡 success** 鈫?鎵撳寘骞?**Publish**锛坄make_latest`锛? 
4. 灏氭湁绔湭瀹屾垚 鈫?閫€鍑虹瓑寰呬笅涓€娆″敜閱? 
5. 鍚?tag 宸插彂甯?鈫?璺宠繃锛堥槻閲嶅锛?
**鎵嬪姩璺緞**浠嶅彲鐢細Actions 鈫?ET Release锛沗publish=false` 鏃跺彧寤?Draft銆?
```bash
gh workflow run "ET Release" --ref main \
  -f source_branch=releases/v2.7.1 \
  -f publish=true
```

---

## 3. 瀹夎鏂规锛堟寜鍦烘櫙锛?
### 3.1 Linux 鎺у埗鍙?/ 鑺傜偣锛堜簩杩涘埗锛屾帹鑽愮敓浜э級

鍏ュ彛锛歚script/easytier-install.sh`锛堟枃妗ｏ細[`../ops/deploy-install.md`](../ops/deploy-install.md)锛?
| 妯″紡 | 瀹夎 | 鍗囩骇 |
|------|------|------|
| Server锛堟帶鍒跺彴锛?| `install --mode server` 鈫?systemd `ET-web` + 鍙€夋湰鏈鸿妭鐐?| `update`锛氫笅 Release 鍖?鈫?鏇挎崲 `/opt/easytier/ET-*` 鈫?閲嶅惎鏈嶅姟锛?*淇濈暀 `et.db`** |
| Client锛堣妭鐐癸級 | `install --mode client` 鈫?systemd `ET-core@鈥 + `--config-server` | 鍚屼笂 `update` |

涓€閿ず渚嬶細

```bash
# 鎺у埗鍙?sudo bash script/easytier-install.sh install --mode server --auto \
  --public-host et.example.com

# 鑺傜偣
sudo bash script/easytier-install.sh install --mode client --auto \
  --server-host 'udp://et.example.com:22020/admin'

# 鍗囩骇锛堝凡瀹夎鏈哄櫒锛?sudo bash script/easytier-install.sh update
```

鍗囩骇鍘熷垯锛?*鍏堝仠鏈嶅姟 鈫?鎹簩杩涘埗 鈫?鍚湇鍔?鈫?healthcheck**锛涙暟鎹簱涓?config 鐩綍涓嶈鐩栥€?
### 3.2 Windows

- 鏃犲ご锛歊elease 涓?`ET-windows-*.zip`  
- 妗岄潰锛歂SIS锛坄ET-gui-*`锛夛紱瀹夎閽╁瓙浼氬仠 `ET-Gui` / 鏃?`easytier-gui` 鏈嶅姟鍚庡啀瑕嗙洊  

### 3.3 Docker

瑙佷粨搴撴牴鐩綍 `docker-compose.yml`锛涘崌绾т互鎹㈤暅鍍?tag + `pull` / `up` 涓轰富锛?*鎸傝浇鍗蜂繚搴?*銆?
---

## 4. 瀹㈡埛绔唴銆屾鏌ユ洿鏂般€嶈兘鍔涚煩闃碉紙鐜扮姸锛?
| 缁堢 | 鐜扮姸 | 澶囨敞 |
|------|------|------|
| Linux 鑴氭湰 | **鏈?* `update` 璇?Release | 鍗婅嚜鍔紝闇€ SSH/杩愮淮鎵ц |
| Windows GUI | **鏃?* Tauri Updater锛坄createUpdaterArtifacts: false`锛?| 闇€閲嶈 NSIS |
| Android | **鏃?* 搴旂敤鍐?GitHub 鏇存柊 | APK 鎵嬪姩鎴栧晢搴?|
| Web UI | **鏃?* 杩?Release 鐨勬娴?| 浠呮湁 i18n 娈嬬暀鏂囨鍙兘 |
| Docker | 闀滃儚鎷夊彇 | 涓嶈蛋 Release zip |

---

## 5. 鍒嗛樁娈碉細宸插仛 / 鍚庣画锛堝伐浣滈噺涓庨闄╋級

| 浼樺厛绾?| 椤?| 鐘舵€?| 椋庨櫓 | 绮椾及 |
|--------|----|------|------|------|
| **P0** | `releases/v*` 涓夌鎴愬姛 鈫?**鑷姩 Publish** Release | **宸茶惤鍦?* | 涓?| 鈥?|
| **P1** | Linux `update` 浣撻獙鎵撶（锛堢増鏈彁绀?changelog锛夛紱Web **浠呮彁绀?*鏈夋柊鐗?閾惧埌 Release | 鍚庣画 | 浣庯綖涓?| 2锝? 浜烘棩 |
| **P2** | Windows GUI Tauri Updater锛堢鍚嶃€佸仠鏈嶅姟銆佸彲鍥炴粴锛?| 鍚庣画 | 涓珮 | 1锝? 浜哄懆 |
| **P3** | Android 搴旂敤鍐呮洿鏂?| 鍚庣画 | 楂?| 1锝? 浜哄懆+ |
| 鈥?| Core/Web 杩涚▼鍐呴潤榛樿嚜鏇挎崲浜岃繘鍒?| **涓嶅仛**锛堥粯璁わ級 | 楂?| 鈥?|

### 5.1 鍚庣画瀹炵幇鏃舵敞鎰?
1. **绛惧悕涓庝緵搴旈摼**锛欸UI/Android 鏇存柊蹇呴』鏍￠獙锛涙棤绉侀挜涓?HTTPS 鍝堝笇鍒欏嬁寮€鑷姩涓嬭浇瀹夎銆? 
2. **鐗堟湰璇箟**锛歡ui / web / core 鏄惁鍚?tag锛涙贩鍗囨椂鐨勫崗璁吋瀹硅鏄庛€? 
3. **鍥藉唴 GitHub 璁块棶**锛氬鎴风妫€娴嬮渶澶嶇敤瀹夎鑴氭湰鐨勯暅鍍忕瓥鐣ャ€? 
4. **杩愯涓浛鎹?*锛氬鐢?NSIS/`sc stop ET-Gui` 缁忛獙锛沇eb 鏇存柊浼樺厛鎸囧紩鍒拌剼鏈€岄潪杩涚▼鑷潃鏇挎崲銆?
---

## 6. Docker 闀滃儚濡備綍鎵撳嚭锛堣繍缁达級

鎵嬪姩锛堝紑鍙戝垎鏀獙璇侊級锛?
```bash
# 鍏堟湁涓€娆℃垚鍔熺殑 ET Linux锛圓rtifact: ET-linux-x86_64锛?gh workflow run "ET Docker" --ref <branch> \
  -f run_id=<Linux_run_id> \
  -f image_tag=dev \
  -f mark_latest=false \
  -f mark_unstable=true
```

姝ｅ紡锛歚releases/*` 涓?Linux 鎴愬姛鍚庯紝`ET Docker` 鍙敱 `workflow_run` 鑷姩鎺ㄩ€侊紱鍖呬綋瀵瑰浠嶅缓璁蛋 **Publish 鍚庣殑 Release**銆?
---

## 7. 鍐崇瓥鎽樿

| 闂 | 绛旀 |
|------|------|
| 浠ュ悗鍖呬粠鍝潵锛?| **GitHub Actions 鈫掞紙姝ｅ紡锛塕elease / GHCR** |
| Artifact 浼氳嚜鍔ㄥ彉 Release 鍚楋紵 | **`releases/v*` 涓婁笁绔悓 commit 鎴愬姛鍚庝細鑷姩 Publish**锛涘叾瀹冨垎鏀粛鍙墜鍔ㄨ窇 `ET Release` |
| 鐢熶骇瑁呬粈涔堬紵 | Linux锛?*瀹夎鑴氭湰 + Release**锛涘鍣細**compose + GHCR**锛涙闈細**GUI NSIS** |
| 鍗囩骇鎬庝箞鍋氾紙浠婂ぉ锛夛紵 | 鎹㈠寘/鎹㈤暅鍍忥紝**淇?db 涓?config**锛涜剼鏈敤 `update`锛孌ocker 鐢?`pull + up` |
| 瀹㈡埛绔唴涓€閿洿鏂帮紵 | **鍚庣画**锛堣 搂5锛夛紱杩戜腑鏈熶笉鍋?GUI/Android 闈欓粯鏇存柊 |

---

## 8. 鍏跺畠鍙€夊寮猴紙鏈疄鐜帮級

1. 瀹夎鑴氭湰澧炲姞 `install --source docker`锛堟媺闀滃儚鍐?systemd + compose锛夈€? 
2. compose 鐢?`.env` 缁熶竴 `IMAGE_TAG` / `CONFIG_SERVER`锛岄伩鍏嶇‖缂栫爜 NAS 涓绘満鍚嶃€? 

---

## 9. 淇璁板綍

| 鏃ユ湡 | 璇存槑 |
|------|------|
| 2026-10-03 | P0锛歚ET Release` Draft 鑱氬悎 |
| 2026-10-03 | `releases/v*`锛氫笁绔?success 鈫?鑷姩 Publish锛沗workflow_run` 椤诲湪 `main` |

