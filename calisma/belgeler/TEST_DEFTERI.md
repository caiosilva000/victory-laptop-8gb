# Test defteri: ne denendi, bizde ne oldu, başkasında ne olmuştu

Makine: RTX 3070 Ti Laptop 8 GB, 16 GB RAM, i7-12700H, Series S profili, hapishane sahnesi.
Kural: her A/B sonucu buraya **aynı gün** yazılır ve repoya gönderilir. Sayı = dünya karesi medyanı (ms, düşük iyi).
"Başkasında" sütunu o değişikliği getirenin kendi iddiasıdır; biz tekrarlamadıysak öyle yazılır.
Ham koşu satırları: KOSULAR.md. Ayrıntılı günlük: STATUS.md. Aday listesi: ADAYLAR.md.

En önemli gerçek (26.11 09:55, kanıtlı): darboğaz **ekran kartı** (%91-100 dolu, kritik yolun ~119/145 ms'si).
Bu yüzden CPU/RAM tarafında kazandıran ayarlar başkasında fps artırsa da bizde kareyi kısaltmıyor.

## Kabul edilenler (tabanda)

| Ne | Kaynak | Başkasında | Bizde | Not |
|---|---|---|---|---|
| RT paketi: BVH_MISS + RT_SKIP_RESOLVE + RT_SKIP_ACTIVATE | cool_fox 8cc31ab3 + bizim BVH_MISS | cool_fox: iddia var, sayı yok | **184 → 150 ms (−%18)**, görüntü aynı | 26.11 11:50 |
| DYNAMIC_SLOTS=524288 | trustno1 taraması, bizim düzeltme | — | reddedilen tablo 562-1357/kare → 0, düşen indirect çizim 229-272 → 0; kare aynı | 09:25, görüntü düzeldi (avlu dolu) |
| CARD_HASH_WIDE (fxceless #1014) | fxceless | 3060 12 GB: hash 35 → 10 ms | geri kopya −%38, kart işi −%43; kare aynı | 25.11 23:14 (o zaman darboğaz gönderimdeydi) |
| İki düzlemli present (siyah ekran düzeltmesi) | ZSlayer PR #10 fikri, Sol portu | Series S siyah ekran düzeliyor | düzeldi, yükleme ekranı görünüyor | 26.11 03:35 |
| warm --used (pipeline ön hazırlığı) | upstream komutu | Nozy: fps değişmedi | fps aynı (172.5), **dünyaya giriş 177 → 105 sn, takılma p90 640 → 309** | Takılma için kabul, fps için değil |
| WINDOWS_BAR_PERCENT=88, WINDOW_KB=256, TAILS, SEND_KEPT_MB=128 | bizim RAM işi | — | kilitli RAM 4726 → 3252 MB, 10 dk sızıntısız | RAM kabulü, fps aynı |
| Windows 10 Agility SDK desteği | bizim | BlatWolf (4060, Win10) için | Win11'de etkisiz, Win10'da çökmeyi önler | 90d3c4f |
| CLAMP_WAITS=0 (victory-scarlett f748fb557a / Trexies 6b358fe, Sol portu 02d7284) | scarlett | Trexies: clamp başı 2 tam bekleme, ~30-50 ms kart iddiası (4080S) | cwb temiz çift **149.3 → 143.0 ms**, p90 250 → 146; cwa: taban bozuk (n=5, 389 ms), clampwaits 145.6 / p90 153 | Küçük kazanç (~%4) ama kareler çok daha düzgün (p90 hep ~145-153). Görüntü aynı (kare_0170s). Bizde clamp'ler zaten toplu, o yüzden 30-50 ms değil (TAHMİN). Tabana alınacak (kw serisi bitince). 26.11 14:10 |
| OWN_UPSCALER=1 + RENDER_SCALE=0.5 (trustno1 afd84de1 ← victory-perf ← ScarlettPort; Neon portu a8ae91f) | RTX 5070: GPU 110 → 82-88 ms | 2 tur × 150 sn, son 60 sn: taban **142/145** · yalnız OWN_UPSCALER **127/132 (−%10)** · OWN+0.5 **112/114 (−%21)** | Bizde de çalıştı çünkü darboğaz kart: piksel işi yarıya inince kart süresi doğrudan düşüyor. RENDER_SCALE tek başına (FSR yolu) açılışta 0xC0000094 sıfıra bölme ile çöküyor → OWN_UPSCALER şart. Görüntü daha yumuşak, avluda hareketli kişilerde hafif hayalet; yüzde 0.6/0.75 seçenekleri sırada. |

## Reddedilenler

| Ne | Kaynak | Başkasında | Bizde | Neden bizde olmadı (kanıt / TAHMİN) |
|---|---|---|---|---|
| CLAMP_WINDOW=1 (Marvisak #3197, Neon portu a478f49; 2 çift × 150 sn, son 60 sn medyanı) | RTX 3070 8 GB Win11: GPU 199 → 170 ms (kare başına ~5100 clamp geçişi olan main'e karşı) | **143→144, 135→146 ms** (kötü/nötr). Sayaç: tabanda kare başına yalnız **~158 clamp geçişi (hepsi batched)**; CLAMP_WINDOW ile 113 batched + 113 pencere (ort. 32 çizim) | Bizde INDIRECT_BATCH=2 clamp geçişlerini zaten ~5100 → ~158'e indirmiş; #3197'nin kazandığı yer bizde yok (3080 Ti karşı ölçümüyle aynı sonuç). **Clamp yönü bizde bitti** — HOIST ve CLAMP_WINDOW ikisi de bu yüzden boş. Kod kapalı kalır. |
| HOIST_CLAMPS=1 (fxceless #1184, Neon portu; yeni diag, 4 tur × 150 sn, son 60 sn medyanı) | 5070 Ti kontrollü A/B: kart 127 → 107 ms (−%16) | taban 151/142/147/141 (ort 145.3) · hoist 145/139/142/148 (ort **143.5, −1.2%**) · INDIRECT_WINDOW=0+hoist 143/138/162/139 (ort 145.5) | İlk iki tur −%3-4 gösterdi, son iki tur geri aldı → gürültü içinde. Neden bizde az: tabanda INDIRECT_WINDOW=1 + BATCH=2 + CLAMP_WAITS=0 clamp maliyetinin çoğunu zaten almış; 5070 Ti testi bunlar olmadan. Kod kapalı kalır (93146f5). INDIRECT_WINDOW=0 tek başına açılışta 3/3 çöküyor, hoist ile çökmüyor. |
| HOIST_CLAMPS=1 (fxceless #1184, Neon portu) — taban üstüne (2 çift × 150 sn) | 5070 Ti kontrollü A/B: kart 127 → 107 ms | **143→146, 148→147 ms** → GEÇERSİZ: kosu.ps1 **diag** derlemesini çalıştırıyor, port yalnız release'e derlenmişti (diag 15:09). hcc/hcd (WINDOW=0) da eski ikiliyle → geçersiz. Diag 16:33'te derlendi, hce/hcf tekrar koşuyor. | Bizim tabanda INDIRECT_WINDOW=1 + INDIRECT_BATCH=2 açık: tek kayıtlı indirect çizimler pencere/toplu yoldan gidiyor, ön listeye alma neredeyse hiç devreye girmiyor. Pencere kapalı üç kollu test (taban / WINDOW=0 / WINDOW=0+HOIST) sürüyor. |
| Pencereleri karta taşıma: RIPE_BLOCKS=4 + CARD_WINDOWS_MB=1536 + CARD_WINDOWS_PERCENT=95 (4 çift × 150 sn, son 60 sn medyanı) | bizim (80% kapısını açan anahtar) | — | **146→137, 146→147, 148→146, 138→141 ms** (ort. −1.8 ms, gürültü içinde); kartta kopya 12→495-665, +300-400 MB taşındı | Taşıma çalışıyor ama kare süresine yansımıyor: darboğaz kartın kendi hesap yükü, pencere verisinin PCIe'den okunması değil. ProbeGather sıçramaları kısaldı (32-42 → 26-28 ms) ama kare başına etkisi ~1 ms. Kapalı kalır. |
| SHADER_LOOP_LIMIT=4096 TEYİT (150 sn) | — | — | lld-loop **152 ms** (son 60 sn); aynı saatteki tabanlar 146 / 149 (lld-taban açılmadı) | İlk çiftteki 172 → 154 farkı tabanın kötü dilimiydi; teyitte kazanç yok → ret kesinleşti. 26.11 15:15 |
| RIPE_BLOCKS=4 + CARD_WINDOWS_MB=768 (150 sn, son 60 sn medyanı) | bizim (VRAM'de yer açma) | — | **146 → 143, 149 → 147 ms** (gürültü içi) | Kart 6.4 → 5.9 GB'a indi (boş doku bloğu 1279 → 838 MB) ama hâlâ %81 dolu: pencere kapısı %80 → pencerelerin çoğu yine taşınmadı. Sonraki: kapıyı ayarlanabilir yaptım (VICTORY_CARD_WINDOWS_PERCENT, 64631e4), %95 ile wp serisi. 26.11 15:15 |
| MESH_LAUNCH=1 (fablinoo/kelvaroq 32c4ab4fe9, Neon portu 84273df) | kelvaroq | **5080: ağır kare p50 198 → 120 ms, GPU %78 → 35** | **146 → 168, 139 → 178 ms** (iki çift, son 60 sn medyanı, her dilimde kötü); kare başına 3346 kayıt amplification aşamasıyla başlatıldı, 0 pipeline bekliyor | Çalışıyor ama yavaş: 'GPU'yu bekleme' 32-40 → 71-78 ms. Ampere'de (30xx) her mesh çizimine amplification aşaması eklemek pahalı; 5080 (Blackwell) bunu ucuza yapıyor + onun eski clamp yolu 58-94 ms tutuyordu, bizimki zaten topluydu (TAHMİN). Kod kalır, varsayılan kapalı. 26.11 14:55 |
| CARD_WINDOWS_MB=768 — **GEÇERSİZ TEST** | GTX 1660 RESULTS.md (Sol) | GTX 1660 6 GB: 271 → 100 ms (kart tamponu + pencere + mip drop birlikte) | 147.2 → 140.1, 148.5 → 149.9 ms (gürültü) | Hiçbir pencere karta gitmedi: shadow.rs window_room_for kapısı 'kullanılan + boyut > bütçenin %80'i ise hayır'; bizde kart hep %85-88 dolu (6.2-6.4/7.25 GB). VRAM'i dolduranlar (log): 12 kart kopyası 1954 MB, doku blok havuzunda 1147 MB BOŞ ayrılmış yer. Yer açılmadan bu ayar çalışmaz. 26.11 14:25 |
| SHADER_LOOP_LIMIT=4096 (ProbeGather döngüsü için) | Marvisak#898 deseni | 4070/4090/3060 TDR'lı dispatch: 183 → 79 ms; scarlett: LOOP_LIMIT=64 kazanç yok | temiz çift **159.3 → 158.8 ms** (fark yok); ikinci çift kirli (CPU 6.4 çekirdek, Sol derlemesi) | ProbeGather döngü sınırına çarpmıyor → yavaşlığı döngü değil. Kalan aday: okuduğu veri sistem RAM'inde (PCIe) — VRAM_WINDOW / VIDEO_BUDGET testi. 26.11 13:45 |
| SEND_BAR=1 | Neon GPU taraması (7900XTX forku) | 7900XTX: kart ~5 ms az | **152 → 180 ms** (temiz çift, +%18); ikinci çift 184 (tabanda takılma, p90 1607) → 174, belirsiz | Kopyalar kartta biter ama CPU kart belleğine veri yolu üzerinden yazıyor: "hazırlık" 95 → 145-160 ms'ye çıktı (log kanıtı) ve kritik yol oldu. 7900XTX masaüstünde tam Resizable BAR var; laptopta bu yazma yavaş (TAHMİN). Kilitli RAM ~230 MB az. 26.11 13:00 |
| INDIRECT_GATHER=1 (fxceless #1171, Sol portu 50ac222) | fxceless #1171 | **3090 Ti: GPU listeleri 56 → 43-45 ms, kare 116 → 105 ms** | **182 → 204 ms (p90 335 → 2625), 151 → 371 ms** (ikinci çift n=4, kare satırları 172-182 ms) | Bizde clamp'ler zaten toplu (batch) çalışıyor, toplayacak ~1000 ayrı geçiş yok; prelude ek bekleme/takılma getirdi (TAHMİN). Düşen çizim sayısı tabanla aynı (271 + ~690), yani bozulma yok, sadece yavaş. 26.11 12:45 |
| TEXTURE_DROP_MIPS=1 | fxceless #610 / soldsawn | soldsawn 4060L 8 GB: DROP_MIPS=2 ile 8-10 fps (A/B yok); #1022 5060L 16 GB RAM: 2-4 fps (A/B yok) | **153 → 168, 160 → 170 ms (+%6-10)** | VRAM'imiz taşmıyor (6.3/7.1 GB), yani kurtaracağı bir şey yok; ek kopya/eşleme işi kaldı (TAHMİN). Kimse temiz A/B yayınlamamış. |
| DISPLAY_MODE=1280x720 | Neon önerisi | — | 177 → 229, 154 → 278 ms | Oyun modu yok sayıyor, düzlemler yine 1920x1080 / 3840x2160 (log kanıtı). Çözünürlük için tek yol RENDER_SCALE. |
| KEEP_FREED=0 | Neon RAM taraması | — | 172 → 168, 159 → 175 ms | Fark gürültü içinde, RAM de düzelmedi. |
| RAM_TRIM (#1240) | fxceless #1240 | RAM rahatlaması iddiası | **155 → 223, 149 → 234 ms (+%45-55)** | Havuzlar hiç dolmadı, her snapshot yeniden ayrılıyor (profil: NtAllocateVirtualMemory %11, kanıt). |
| MEMORY_MAP=0 | bizim şüphe | — | 161 → 172, 156 → 167 ms | Kazanç yok; uzun kareler yalnız dünyaya girişin ilk 10-20 sn'si. |
| JOIN_PARTS=8 | fxceless | 4070: hız iddiası | 128-149 ms (hızlı) ama **görüntü bozuk** (bulanık, yıkanmış) | Hız ne olursa olsun ret. JOIN_PARTS=2 park. |
| CARD_BUFFERS_MIN_MB=1 | fxceless #413 reçetesi | **3070 Ti 8 GB: 2.05 → 4.62 fps** | **150 → 307 ms (2 kat yavaş)** | Karşılaştırma işi karta taşınıyor, kart işi 42 → 201 ms (log kanıtı). #413'te darboğaz CPU karşılaştırmasıydı, bizde kart. |
| RAW_VIEWS=0 | profil bulgusu | — | 149.7 vs 149.9 ms | Fark yok. |
| LTO thin | soldsawn Cargo.toml | soldsawn: ayrı ölçüm yok | 173 → 199 ms (+%15) | Gürültüden büyük; geri alındı. |
| Shader tablo yaması (e04c0781 elle) | fxceless | — | görüntü yok, RAM kazancı yok | Geri alındı. |
| TEXTURE_BUDGET_MB=768 | bizim | — | 149 ms ama **görüntü bozuk** (doku eksik, oda kararmış) | 1024+ kalır. |
| INPLACE=1 ("sans copie") | Nozy yayını | **Nozy: 16-18 fps** | 490. sn'de **cihaz sıfırlandı** (0x887A0005), yalnız 137 pencere yerinde | Nozy'nin 64 GB'ı var; bizde 16 GB'ta kilitli RAM 5.3 GB'a çıktı (kanıt). |
| COPY_MODE=plain | fxceless | — | 60. sn'de **çöktü** (0xC0000005) | Ham kopya eşlenmemiş sayfaya çarpıyor (Opus uyarısı doğrulandı). |
| WIDE_MERGE=1 (#1066) | fxceless | — | +18 ms (+%7) | Kopya işi arttı. |
| WINDOW_KB=128 | bizim | — | 263-272 → 296 ms | Daha çok parça; 256 kalır. |
| WIDE_REFRESH=1 | fxceless | — | fark yok | Kapalı. |
| cool_fox derlemesinin tamamı | codec-d/cool_fox | "çok hızlı" iddiası, ölçüm yok | **~2.5 kat yavaş + siyah ekran** | Siyah ekran: premultiplied alfa varsayımı (bizimki düz alfa). Repo silindi; iyi parçaları tek tek alınıyor. |


## Teşhis ölçümleri (kabul/ret değil, nereye bakacağımızı gösterir)

| Ne | Bizde | Anlamı |
|---|---|---|
| GPU_TIMING=1 + DRED (gt-teshis, 26.11 13:05) | tek komut baskın: indirect compute **3592426e3e9142d8 = ProbeGather** (ışık probu ışın toplama, XTP shader-roles), tek başına 32.5 / 42.5 ms | Başkasında: 4060 Ti 8 GB ort 9 ms, en kötü 40.7 ms (fxceless #10). Büyük listelerdeki "doku yükleme 8-30 ms" ölçüm hatası: liste başına 4096 damga, kalan komutlar sonuncuya yazılıyor (host.rs STAMPS). |
| SKIP_PIPELINES=3592426e3e9142d8 (tavan ölçümü) | temiz çift skc: **149.9 → 134.6 ms (−%10)**, p90 587 → 150 (takılmalar kayboldu); diğer çiftler 145/146 (tabanları kirli) | Kazanç tavanı ~15 ms + takılmalar. **Görüntü bozuluyor** (duvar posterlerinde renkli bozuk kareler) → atlamak çözüm değil; shader'ın kendisi hızlanmalı (LOOP_LIMIT testi). |
| GPU_TIMING + pencereler kartta (gtw-teshis-win95, 26.11 15:28; RIPE4, WIN1536, PERCENT95) | ProbeGather kare başına **~1 ms** (eski koşuda da 0.6-1.7 ms); 32-42 ms olan tek tük sıçramalardı, şimdi en uzunu 26-28 ms. Kart süresi tek komutta değil, 13-22 listelik birçok gruba yayılmış (her biri 9-18 ms/kare). | **Düzeltme:** ProbeGather kalıcı darboğaz değil, ara sıra takılma kaynağı. Pencereleri karta taşımak sıçramayı ~%30 kısalttı. Atlayınca gelen 15 ms'lik kazanç büyük ihtimalle takılmaların kaybolmasından geliyor. #2705 CARD_COPIES bu yüzden öncelikten düştü. |

## Belirsiz / tekrar gerek

| Ne | Bizde | Not |
|---|---|---|
| BVH_MISS tek başına | 176/162 vs 208 ms | Bir taban bozuktu; paketle birlikte kabul edildi. |
| ASYNC_SUBMIT=1 | 154 → 148 ms (1 temiz çift) | Küçük; ikinci temiz çift gerek. |
| CBV_DIRECT=1 | 267 → 255 ms (eski taban) | Gürültü eşiğinin altında; tekrar gerek. |

## Sırada (STATUS.md "TEST LİSTESİ")

fxceless #1171 indirect gruplama (Sol portluyor; 3090 Ti: kart 56 → 43 ms) → #1227 CARD_SYNC_BATCH (kartta 29 → 6 ms) → SEND_BAR=1 → GPU_TIMING teşhisi → RENDER_SCALE 480/540/720p.
Ölçüm serisinden önce `shaders` + `warm --used` tazelenir.
