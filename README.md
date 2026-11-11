# DiscoLab

Teste de discos em Rust para **Linux, macOS, Windows e Android** — saúde (SMART),
benchmark e varredura de superfície, reunindo o melhor do HD Tune, Victoria,
HD Sentinel e CrystalDiskMark, com foco em resultados **honestos**.

| Aba | O que faz |
|---|---|
| **Saúde** | Nota 0–100 %, vida útil restante, temperatura, horas, TB escritos, diagnóstico explicado e autotestes SMART (curto/longo) |
| **SMART** | Tabela completa de atributos ATA com explicação em português, log de saúde NVMe e desgaste eMMC/UFS |
| **Benchmark** | SEQ 1M Q8/Q1 e RND 4K Q32/Q1 (leitura e escrita) com MB/s, IOPS e latência p50/p99/p99.9/máx, gráfico ao vivo e teste de **escrita sustentada** que mostra onde o cache SLC acaba |
| **Superfície** | Varredura **somente leitura** com mapa de blocos colorido (estilo Victoria), curva de velocidade ao longo do disco (estilo HD Tune) e releitura dos blocos suspeitos |
| **Relatório** | Texto pronto para copiar e salvar em `.txt` + `.json` |

## Por que os resultados são mais confiáveis

**Contra o "rigor demais" (Victoria):**
- Um bloco lento só vira problema se **continuar lento ao ser relido** (até 3 vezes). Picos causados pelo
  sistema operacional aparecem como "transitórios", sem afetar a nota.
- Erros de CRC (ID 199) são apontados como **cabo/porta**, não como defeito do disco.
- Poucos setores realocados e estáveis viram "observação", não "disco ruim".
- Entradas no log de erros NVMe sem erros de mídia são tratadas como informativas.

**Contra o "otimismo demais/engano" (CrystalDiskMark, HD Tune):**
- E/S **sem cache do SO** (`O_DIRECT` / `F_NOCACHE` / `FILE_FLAG_NO_BUFFERING`); se não for possível, o app avisa.
- Dados **aleatórios e incompressíveis**, com cada página de 4 KiB alterada a cada escrita
  (controladores que comprimem/deduplicam não inflam o resultado).
- **Escrita sustentada** de vários GB: mostra pico × velocidade real depois que o cache SLC/DRAM enche.
- **Latência por operação** (p99/máx): médias escondem travadas; percentis não.
- O status "SMART PASSED" do firmware **não basta**: setores pendentes/incorrigíveis reduzem a nota mesmo assim.
- Valores brutos "empacotados" da Seagate (IDs 1 e 7) não geram alarme falso.

## Requisitos

O SMART completo usa o **smartmontools** (`smartctl`), a implementação de passthrough ATA/SCSI/NVMe mais
testada que existe. O DiscoLab procura o `smartctl` no PATH, em locais comuns e **na mesma pasta do executável**
(basta distribuir os dois juntos).

| Sistema | Instalar smartmontools | Permissões |
|---|---|---|
| macOS | `brew install smartmontools` | SMART do SSD interno funciona sem sudo; superfície exige `sudo` |
| Linux | `sudo apt install smartmontools` (ou dnf/pacman) | SMART e superfície exigem root |
| Windows | instalador em smartmontools.org | Executar como Administrador |
| Android | — | Sem root: benchmark + eMMC/UFS (quando o sistema expõe). Com root: tudo |

Sem `smartctl`, o app usa alternativas: contadores de confiabilidade do Windows (`Get-StorageReliabilityCounter`)
e o desgaste de eMMC/UFS via sysfs no Linux/Android.

## Compilar e rodar

```bash
cargo run --release -p discolab          # app gráfico
cargo run --release -p discolab-cli -- list
```

Linux (dependências do app gráfico):

```bash
sudo apt install libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev
```

Para usar a varredura de superfície no Linux/macOS: `sudo ./target/release/discolab` (ou a CLI com sudo).

### CLI

```
discolab-cli list
discolab-cli health disk0            # nvme0n1, sda, PhysicalDrive0…
discolab-cli health sda --json
discolab-cli bench /mnt/dados --size 1024 --secs 5 --sustained 16
sudo discolab-cli scan sda --start 0 --end 100 --block 128
discolab-cli selftest sda short
```

### Android

```bash
./scripts/build-android.sh     # precisa de rustup + Android SDK/NDK + Java 17
adb install -r target/release/apk/DiscoLab.apk
```

No Android sem root, o benchmark roda no armazenamento interno do app; o SELinux bloqueia o acesso direto
ao chip interno do celular.

**Discos em adaptador USB-OTG (sem root):** conecte o HD/SSD pelo adaptador, abra o DiscoLab e toque em
"Permitir" quando o Android pedir acesso. O app conversa direto com o adaptador (USB Mass Storage Bulk-Only):
- HD/SSD SATA: SMART via SAT (ATA PASS-THROUGH 16/12) — funciona na maioria dos adaptadores JMicron, ASMedia, Realtek e VIA.
- SSD NVMe: passthrough das pontes Realtek RTL9210/9220, JMicron JMS583 e ASMedia ASM2362.
- Varredura de superfície somente leitura de todo o disco.
- Adaptadores que só falam UAS ou bloqueiam passthrough: a superfície funciona, o SMART não.
- Durante o acesso direto o Android desmonta o volume; reconecte o adaptador ao terminar.

### macOS (.app)

```bash
./scripts/package-macos.sh     # gera target/DiscoLab.app com o smartctl embutido
```

### Compilação para todos os sistemas

`.github/workflows/build.yml` compila e testa em Linux, Windows e macOS e gera o APK Android a cada push
(artefatos na aba Actions do GitHub).

## Estrutura

```
crates/core   biblioteca: discos, SMART, diagnóstico, benchmark, superfície, relatório
crates/app    interface egui/eframe (desktop + Android)
crates/cli    linha de comando
```

Testes: `cargo test -p discolab-core` · capturas da interface:
`SHOTS=/tmp/shots cargo test --release -p discolab --test screens -- --ignored`.

## Segurança dos dados

- A varredura de superfície é **somente leitura**. Não há teste destrutivo de escrita no disco bruto.
- O benchmark escreve apenas arquivos temporários na pasta escolhida e os apaga ao final (inclusive se cancelado).
- Escrita sustentada grande consome ciclos de escrita do SSD; use com moderação (8–32 GiB costuma bastar).
