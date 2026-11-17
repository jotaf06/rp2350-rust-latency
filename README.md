# rp2350-rust-latency

Firmware em Rust para medir **latência de interrupção, duração da ISR e latência de
escalonamento** com Embassy e RTIC, em duas placas:

| Placa | Chip | Núcleo | Clock padrão | Feature | Alvo |
| --- | --- | --- | --- | --- | --- |
| Raspberry Pi Pico 2 | RP2350A | Cortex-M33 (modo ARM) | 150 MHz | `rp2350` (padrão) | `thumbv8m.main-none-eabihf` |
| Raspberry Pi Pico | RP2040 | Cortex-M0+ | 125 MHz | `rp2040` | `thumbv6m-none-eabi` |

O código-fonte é o mesmo; o pouco que muda entre os chips está em `src/chip.rs`. Nos scripts,
a placa é escolhida pela variável `CHIP` (`rp2350` se omitida). Medir nas duas permite comparar
os núcleos (seção 5).

Para saber qual placa você tem: em BOOTSEL, `lsusb` mostra `2e8a:000f` no RP2350 e
`2e8a:0003` no RP2040; `picotool info -a` também informa o chip e o encapsulamento.

**RP2350B**: o build `rp2350` (feature `rp235xa` do embassy-rp) roda também no RP2350B (QFN80,
placas compatíveis de terceiros), porque o firmware só usa GPIO 0–29, que existem nos dois. As
medições "RP2350" deste repositório foram feitas numa placa **RP2350B**; no resumo ela aparece
como `rp2350`. Placas sem LED no GPIO25 precisam de um LED externo **com resistor em série**
(220–330 Ω) se você quiser ver o indicador de coleta.

## 1. O que mede e a relação com o artigo

Referência: Vandervelden et al., *Overview of Embedded Rust Operating Systems and Frameworks*,
Sensors 2024, 24, 5818 ([doi](https://doi.org/10.3390/s24175818)). O artigo mediu Tock, RTIC e
Embassy numa nRF52840-DK (Cortex-M4 a 64 MHz), com o sistema **ocioso**, e reportou só
**média ± IC 95 %** (Tabela 3) e a memória de um blink (Tabela 4).

Este projeto responde a duas perguntas:

1. **Os números se reproduzem em outro hardware?** Cenário S0 (ocioso) no RP2350 a 150 MHz
   e/ou no RP2040 a 125 MHz.
2. **O que acontece com o pior caso quando há carga?** Cenário S1, que o artigo não mediu.
   Uma tarefa de baixa prioridade ocupa a CPU por `L` µs sem `.await`. Hipótese: no executor
   cooperativo do Embassy o **máximo** da latência cresce até ≈ `L`; com um `InterruptExecutor`
   (ou com RTIC, que é preemptivo) ele continua perto do caso ocioso.

Método: um slice de PWM roda livre e é, ao mesmo tempo, **fonte do evento e relógio**. O estouro
do contador (*wrap*) gera a interrupção `PWM_IRQ_WRAP_0`; como o contador volta a zero no evento,
ler o contador dá o tempo decorrido desde o evento, com resolução de 1 ciclo de clock (6,7 ns).

| Leitura do contador | Onde | Métrica |
| --- | --- | --- |
| `t_isr_start` | primeira instrução útil da ISR | `irq_lat = t_isr_start` |
| `t_isr_end` | última instrução da ISR, depois de sinalizar a tarefa | `isr_dur = t_isr_end − t_isr_start` |
| `t_task` | primeira instrução da tarefa ao acordar | `sched_lat = t_task − t_isr_end`, `total = t_task` |

Cada lote tem 2000 eventos, guardados num buffer estático. **Nada é enviado durante a coleta**;
o lote sai pela USB serial depois. Não é preciso analisador lógico nem debug probe.

## 2. Material

- Raspberry Pi Pico 2 (RP2350A) ou Raspberry Pi Pico (RP2040), e cabo USB de dados.
  Os pinos abaixo são os mesmos nas duas. Na Pico **W** e na Pico 2 **W** o LED não fica no
  GPIO25 (é ligado ao chip de Wi-Fi), então ele não acende; a medição não depende dele.
- Opcional: analisador lógico ou osciloscópio, para ver os mesmos eventos por fora:

| Pino | Sinal |
| --- | --- |
| GPIO0 | saída do PWM: sobe no evento (50 % de duty) |
| GPIO2 | alto durante a ISR |
| GPIO3 | alterna a cada ativação da tarefa medida |
| GPIO25 | LED: aceso durante a coleta de um lote |

## 3. Instalação

```sh
rustup target add thumbv8m.main-none-eabihf   # RP2350
rustup target add thumbv6m-none-eabi          # RP2040
rustup component add llvm-tools          # só para tools/sizes.sh
sudo apt install python3-serial python3-matplotlib   # matplotlib é opcional
```

`picotool` 2.x: `sudo apt install picotool` nas versões do Ubuntu/Debian que o empacotam. No
Ubuntu 24.04 não há pacote; compile-o (precisa de `cmake`, `g++` e `libusb-1.0-0-dev`):

```sh
mkdir -p ~/pico && cd ~/pico
git clone --depth 1 -b 2.1.1 https://github.com/raspberrypi/pico-sdk.git
git clone --depth 1 -b 2.1.1 https://github.com/raspberrypi/picotool.git
cd picotool && mkdir build && cd build
cmake .. -DPICO_SDK_PATH=$HOME/pico/pico-sdk && make -j$(nproc)
sudo make install
sudo cp ../udev/99-picotool.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger
```

Para usar o `picotool` e a porta serial sem `sudo`: instale as regras udev do picotool e entre no
grupo `dialout` (`sudo usermod -aG dialout $USER`, depois faça login de novo).

Sem `picotool`, `tools/flash.sh` gera um `.uf2` (`tools/elf2uf2.py`) para arrastar ao disco
`RP2350` (ou `RPI-RP2`, no RP2040). Esse caminho alternativo **não foi testado em placa**; o `picotool` é o caminho principal.

## 4. Gravar e coletar

Primeira gravação: segure **BOOTSEL** ao conectar a placa. Depois disso, `tools/flash.sh` reinicia
sozinho o firmware em modo BOOTSEL (abre a serial a 1200 baud), sem precisar do botão.

```sh
tools/flash.sh <binário> [perfil]        # perfil: release (opt-level 3, padrão) ou release-z
CHIP=rp2040 tools/flash.sh <binário>     # mesma coisa no Raspberry Pi Pico (RP2040)
python3 tools/analyze.py --port /dev/ttyACM0
```

Equivalente manual: `cargo run --release --bin <binário>` (o runner é
`picotool load -u -v -x -t elf`) ou `cargo run --profile release-z --bin <binário>`. No RP2040,
use os apelidos de `.cargo/config.toml`: `cargo run-rp2040 --release --bin <binário>`,
`cargo build-rp2040 ...` e `cargo clippy-rp2040 ...` (equivalem a
`--no-default-features --features rp2040 --target thumbv6m-none-eabi`).

**Toda a tabela abaixo vale para as duas placas**: no RP2040, prefixe os comandos de gravação e o
`tools/sizes.sh` com `CHIP=rp2040`. O `analyze.py` não precisa de opção: cada lote informa o chip
(`chip=` no `META`).

`analyze.py` espera os lotes, guarda o log bruto em `results/raw/` e **refaz** `results/summary.md`
com tudo o que já foi capturado. Por padrão captura um ciclo completo de lotes do binário;
`--batches N` fixa a quantidade e `--append` acumula em vez de substituir a captura anterior.

| Binário | Cenário | Lotes por ciclo | Gravar | Coletar |
| --- | --- | --- | --- | --- |
| `s0_thread_idle` | S0: tarefa no executor de thread, ocioso | 1 | `tools/flash.sh s0_thread_idle` | `python3 tools/analyze.py --port /dev/ttyACM0` |
| `s0_irq_idle` | S0: tarefa num `InterruptExecutor`, ocioso | 1 | `tools/flash.sh s0_irq_idle` | idem |
| `s1_thread_load` | S1: tarefa e carga no **mesmo** executor de thread | 3 (L = 10, 100, 1000 µs) | `tools/flash.sh s1_thread_load` | `python3 tools/analyze.py --port /dev/ttyACM0 --batches 3` |
| `s1_irq_load` | S1: tarefa no `InterruptExecutor`, carga no de thread | 3 | `tools/flash.sh s1_irq_load` | idem, `--batches 3` |
| `baseline_bare_irq` | piso do hardware: mesma ISR, sem executor | 1 | `tools/flash.sh baseline_bare_irq` | `python3 tools/analyze.py --port /dev/ttyACM0` |
| `s2_thread_ntasks` | latência × N tarefas extras (Figura 4) | 5 (N = 1, 2, 4, 8, 16) | `tools/flash.sh s2_thread_ntasks` | `... --batches 5` |
| `rtic_latency` | RTIC 2: S0 e S1 num binário só | 4 (L = 0, 10, 100, 1000 µs) | `tools/flash.sh rtic_latency` | `... --batches 4` |
| `blink`, `rtic_blink` | só memória (Tabela 4) | — | — | `tools/sizes.sh` |

Para repetir tudo com `opt-level = "z"`, acrescente `release-z`: `tools/flash.sh s0_thread_idle release-z`.
Para rodar a 64 MHz (clock do artigo): `tools/flash.sh s0_thread_idle release --features clk-64mhz`
(no RP2040: `CHIP=rp2040 tools/flash.sh s0_thread_idle release --features clk-64mhz`).

Roteiro mínimo para o seminário (P0): `s0_thread_idle`, `s0_irq_idle`, `s1_thread_load`,
`s1_irq_load` em `release`, depois `tools/sizes.sh` e `python3 tools/analyze.py` (sem argumentos,
só para refazer o resumo com a Tabela C).

Testar o script sem placa: `python3 tools/analyze.py --file tools/sample.log`. Esse log é
**sintético** (gerado por `tools/gen_sample_log.py`); a saída vai para `results-synthetic/`, nunca
para `results/`.

Se nada chegar: o LED deve piscar a cada lote; confira a porta com `ls /dev/ttyACM*`; no lote com
`L = 1000 µs` a coleta leva uns 5 s. A placa só transmite com a porta aberta e recomeça o lote
quando a porta é aberta no meio de uma coleta.

### Firmware Zephyr

`zephyr-latency/` é uma aplicação Zephyr (em C) com o mesmo método, os mesmos pinos e o mesmo
protocolo de saída; o `analyze.py` lê os lotes dela sem opção nenhuma. Um binário faz o caso
ocioso e as três cargas (L = 0, 10, 100, 1000 µs), como o `rtic_latency`, em duas variantes:

| Modo | Thread medida × carga | Contrapartida no Rust |
| --- | --- | --- |
| `preempt` | preemptivas, a medida com prioridade maior | RTIC / `InterruptExecutor` |
| `coop` | cooperativas, mesma prioridade | executor de thread do Embassy |

```sh
tools/zephyr.sh preempt [release|release-z]     # compila e grava (RP2350)
python3 tools/analyze.py --port /dev/ttyACM0 --batches 4
NOFLASH=1 tools/zephyr.sh coop release-z        # só compila
```

#### Requisito: workspace do Zephyr

O Zephyr **não faz parte deste repositório**. Para compilar `zephyr-latency/` é preciso um
workspace west com o Zephyr e o Zephyr SDK instalados (testado com o Zephyr 4.5.99, do `main`, e
o Zephyr SDK 1.0.1). O script espera o `west` num ambiente Python em `$ZEPHYR_WS/.venv`, como no
guia oficial (<https://docs.zephyrproject.org/latest/develop/getting_started/>):

```sh
python3 -m venv ~/zephyrproject/.venv
source ~/zephyrproject/.venv/bin/activate
pip install west
west init ~/zephyrproject && cd ~/zephyrproject && west update
west zephyr-export && west packages pip --install
# + Zephyr SDK (passo "Install the Zephyr SDK" do guia)

export ZEPHYR_WS=~/zephyrproject     # padrão do tools/zephyr.sh; mude se o workspace estiver em outro lugar
```

O script usa o workspace **só para compilar** (não altera nada nele); o build fica em
`build-zephyr/`. Zephyr 4.5.99 + Zephyr SDK 1.0.1. Perfis:
`release` = `-O3`, `release-z` = `-Os` (GCC, enquanto o Rust usa LLVM). A saída é USB CDC ACM
(VID:PID `c0de:cafe`, produto `zephyr-latency`), e o reboot em BOOTSEL por 1200 baud funciona
como nos binários Rust. Um build com `-DEXTRA_CFLAGS=-DLAT_DEBUG` (passado depois do perfil)
imprime o andamento em linhas `#` e reinicia em BOOTSEL se houver falha fatal.

## 5. Como ler `results/summary.md`

`results/` **não é versionado**: é criado na sua máquina pelo `tools/analyze.py`, a partir das
capturas guardadas em `results/raw/`. Os números citados na seção 7 vêm das medições feitas para o
seminário.

O resumo tem uma seção por chip e clock (`## RP2350 (Cortex-M33) — clk_sys = 150 MHz`,
`## RP2040 (Cortex-M0+) — clk_sys = 125 MHz`, ...), cada uma com as tabelas A e B.

- **Tabela A** — S0 (ocioso): nossas médias ao lado das do artigo, em µs e em ciclos de clock.
  É a reprodução da Tabela 3. Inclui o máximo, que o artigo não informa.
- **Tabela B** — S1 (com carga): média, p99 e **máximo** da latência total por executor e por `L`.
  É a tabela central: compare a coluna “máximo” entre o executor de thread e o
  `InterruptExecutor`. `máx. / L ≈ 1` significa que a tarefa esperou o trecho inteiro da carga.
- **Comparação RP2350 × RP2040** — aparece quando o mesmo cenário (binário, perfil, `L`, `N`) foi
  medido nas duas placas, cada uma no seu clock padrão (ou as duas a 64 MHz). Dá média e máximo
  em µs e a média em ciclos. Em µs a diferença mistura clock e núcleo; a razão em ciclos isola o
  efeito do núcleo (M0+ × M33), da memória e do código gerado.
- **Tabela C** — memória do blink × Tabela 4 do artigo (precisa de `tools/sizes.sh`; rode também
  `CHIP=rp2040 tools/sizes.sh` para ter as duas placas no mesmo `sizes.csv`).
- **Estatísticas completas** — por cenário e métrica: n, mínimo, média, IC 95 %, desvio, p50, p99
  e máximo, com e sem aquecimento.
- **Aquecimento**: as 50 primeiras amostras de cada lote são marcadas, não descartadas. Médias são
  dadas sem elas; máximos, com elas (falha de cache também é pior caso).
- **Descartadas**: eventos em que a tarefa só rodou depois do evento seguinte (ou muito perto
  dele), quando o contador já não vale como relógio. Deve ser 0; se não for, o máximo real é
  maior que o reportado.

Também são gerados `results/<chip>_<bin>_<perfil>.csv` (as mesmas estatísticas, em µs e ciclos),
`results/latency_<chip>.png` (média × máximo por cenário) e um histograma por cenário. Os logs
brutos ficam em `results/raw/<chip>_<bin>_<perfil>.log`, então capturas das duas placas convivem.

## 6. Diferenças em relação ao artigo

- **Hardware e clock**: RP2350A (Cortex-M33) a 150 MHz ou RP2040 (Cortex-M0+) a 125 MHz, contra
  nRF52840 (Cortex-M4) a 64 MHz. Valores absolutos em µs valem pouco para comparação. A
  comparação em **ciclos** é só indicativa: os núcleos são diferentes e os dois RP executam da
  flash externa por cache XIP, enquanto o nRF52840 tem flash interna. O M0+ é o mais distante do
  artigo: pipeline de 2 estágios, sem BASEPRI (o RTIC usa máscara de interrupções), sem
  instruções atômicas de leitura-modificação-escrita e só 4 níveis de prioridade no NVIC.
- **Evento interno**: o artigo disparava a interrupção por um pino de entrada; aqui o evento é o
  *wrap* de um PWM. O sincronizador de entrada do GPIO (alguns ciclos) não entra na medida.
- **ISR própria**: o artigo usou a ISR de GPIO do HAL de cada sistema; aqui a ISR é mínima e
  escrita à mão (lê o contador, limpa o flag, sinaliza a tarefa). Por isso a “duração da ISR” do
  artigo tende a ser maior que a nossa.
- **Medição interna**: o artigo mediu com osciloscópio; aqui a medida é o contador do PWM. As
  leituras ficam alguns ciclos depois da entrada da ISR e antes da saída (prólogo e epílogo
  gerados pelo compilador), e a instrumentação (duas escritas de GPIO, um contador de
  sequência) entra na duração da ISR.
- **Estatística**: reportamos mínimo, máximo, p50 e p99, além de média e IC 95 %.
- **Cenário com carga (S1)**, tipo de executor declarado e tarefas extras definidas (S2): não
  existem no artigo.
- **Versões**: Embassy 0.10 e RTIC 2.4 em rustc 1.93, contra Embassy 0.1.0, RTIC 2.0.0 e um
  nightly de 2023.

## 7. Hipóteses e resultados

### 7.1 Hipóteses iniciais

Escritas antes de qualquer medição:

- **S0**: executor de thread e `InterruptExecutor` com médias parecidas, na casa de poucos µs.
- **S1 com executor de thread**: o máximo da latência total cresce aproximadamente com `L`
  (≈ 10, 100 e 1000 µs), e a média fica perto de `L / 2`.
- **S1 com `InterruptExecutor`**: o máximo fica próximo do S0, qualquer que seja `L`.
- **RTIC**: como no `InterruptExecutor`, independente de `L`.

### 7.2 Resultados no RP2350 (placa RP2350B, 2026-11-17) — Rust

**Estes são os resultados usados no seminário.** Todos os lotes com 2000 eventos e **0
descartados**; números completos em `results/summary.md`.

**S0, sistema ocioso, 150 MHz** — médias, em µs e (ciclos):

| | Interrupção | Duração ISR | Escalonamento | **Total** `release` | **Total** `release-z` |
| --- | --- | --- | --- | --- | --- |
| Sem executor (`baseline_bare_irq`) | 0,167 (25) | 0,107 (16) | 0,196 (29) | **0,47 (70)** | 0,48 (72) |
| RTIC 2 | 0,220 (33) | 0,387 (58) | 0,413 (62) | **1,02 (153)** | 1,17 (175) |
| Embassy, thread | 0,207 (31) | 0,627 (94) | 0,667 (100) | **1,50 (225)** | 2,03 (305) |
| Embassy, `InterruptExecutor` | 0,207 (31) | 0,680 (102) | 0,680 (102) | **1,57 (235)** | 2,13 (320) |
| Artigo, RTIC (nRF52840, 64 MHz) | 0,668 | 2,207 | 1,135 | 4,01 | 5,25 |
| Artigo, Embassy | 0,822 | 4,844 | 4,164 | 9,83 | 13,3 |

(decomposição em `release`). A ordem é a do artigo: sem executor < RTIC < Embassy. A latência de
interrupção é praticamente a mesma em todos (é do hardware); a diferença está na ISR e no
escalonamento. Depois do aquecimento, cada configuração dá o mesmo número de ciclos em todos os
eventos (variação de 1–4 ciclos); os máximos maiores vêm só das primeiras amostras (cache frio).

**64 MHz (clock do artigo)**: os ciclos são idênticos aos de 150 MHz em todas as configurações.
No mesmo clock do artigo: Embassy thread 3,52 µs (artigo: 9,83) e RTIC 2,39 µs (artigo: 4,01),
em `release`.

**S1, com carga, `release`** — latência total, média / máximo sem aquecimento:

| Executor da tarefa medida | L = 10 µs | L = 100 µs | L = 1000 µs |
| --- | --- | --- | --- |
| Embassy, thread | 16,2 / 21,2 µs | 148 / 201 µs | 1514 / **2001 µs** |
| Embassy, `InterruptExecutor` | 1,58 / 1,59 µs | 1,58 / 1,59 µs | 1,56 / 1,59 µs |
| RTIC 2 | 1,03 / 1,04 µs | 1,03 / 1,05 µs | 1,00 / 1,04 µs |

Em `release-z` o quadro é o mesmo (thread: 1507 / 2002 µs com L = 1000).

- **Confirmado**: `InterruptExecutor` e RTIC não dependem de `L`; o executor de thread sim.
- **Refutado em parte**: no executor de thread a latência fica entre **L e 2L** (média ≈ 1,5 L),
  e não entre 0 e L. Causa: a fila de prontos do `embassy-executor` é uma **pilha** (LIFO dentro de
  cada rodada, `embassy-executor/src/raw/run_queue.rs`). A ISR acorda a tarefa medida durante o
  trecho da carga; ao terminar o trecho, o `yield_now()` da carga a reacorda e a empilha **por
  cima** da tarefa medida. Na rodada seguinte a carga roda mais um trecho inteiro antes.

**S2, tarefas extras, `release`** (Figura 4 do artigo): média de 1,50 a 1,78 µs e p99 de 1,50 a
9,65 µs para N = 1 a 16. A média quase não muda; a cauda cresce com N.

**Memória (blink, `release-z`)**: Embassy 2896 bytes e RTIC 3140 bytes, contra 43964 e 6904 no
artigo. O `.bss` do Embassy (336 bytes contra 34264) confirma que o valor do artigo vinha da arena
de tarefas configurada, e não do Embassy em si.

### 7.3 Zephyr no RP2350 (2026-11-17)

Mesmo método e protocolo (seção 4, "Firmware Zephyr"), Zephyr 4.5.99. Latência total a 150 MHz, média / máximo sem
aquecimento, em µs:

| | Ocioso | L = 10 µs | L = 100 µs | L = 1000 µs |
| --- | --- | --- | --- | --- |
| Zephyr, threads preemptivas (`release`) | 5,49 / 6,09 | 6,97 / 7,69 | 6,93 / 7,67 | 6,97 / 7,68 |
| Zephyr, threads preemptivas (`release-z`) | 5,44 / 5,44 | 7,66 / 7,89 | 7,67 / 7,92 | 7,68 / 7,92 |
| Zephyr, threads cooperativas (`release`) | 2,59 / 2,59 | 8,0 / 14,0 | 53 / 104 | 502 / **1003** |
| Zephyr, threads cooperativas (`release-z`) | 2,68 / 2,68 | 7,7 / 12,0 | 53 / 105 | 503 / **1005** |

- **Cooperativo: máximo ≈ L e média ≈ L/2**, exatamente a hipótese inicial, que o Embassy
  (≈ 2L) não confirmou. A fila de prontos do Zephyr é FIFO por prioridade: o `k_yield()` põe a
  carga **atrás** da thread medida. Mesma situação, pior caso 2× diferente, só pela estrutura da
  fila de prontos.
- **Preemptivo: não depende de `L`** (como RTIC e `InterruptExecutor`), mas é 3–5× mais lento em
  valor absoluto. O custo está **dentro da ISR**: 570–990 ciclos de duração, contra 58 do RTIC e
  ≈ 100 do Embassy. É o `k_sem_give()` decidindo, dentro da interrupção, a preempção da thread em
  execução, mais o wrapper genérico de ISR do Zephyr. A troca de contexto em si custa 105–116
  ciclos. Com carga, a ISR fica ≈ 220–330 ciclos mais longa que em repouso.
- A latência de interrupção do Zephyr (≈ 48 ciclos) é um pouco maior que a do Rust (≈ 31): o
  wrapper de ISR do Zephyr roda antes do código da aplicação.
- O build preemptivo `release` varia ±80 ciclos em repouso (latência de interrupção entre 50 e
  160 ciclos); o `release-z` não. Hipótese: posição do código no cache XIP, como no RP2040 a
  64 MHz. Não confirmado.

### 7.4 RP2040 (arquivado)

O projeto também roda na Raspberry Pi Pico (RP2040), e o pipeline Rust foi medido nela, mas **esses
dados não são usados no seminário** e não estão no repositório. O Zephyr não foi medido no
RP2040.
