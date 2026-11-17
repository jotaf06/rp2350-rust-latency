#!/usr/bin/env python3
"""Collect and analyse latency batches from the rp2350-rust-latency firmware
(RP2350A / Pico 2 and RP2040 / Pico builds).

Input is the text protocol the firmware prints over USB serial:

    META,chip=...,bin=...,profile=...,clk_sys_hz=...,pwm_div=...,pwm_top=...,load_us=...,n=...
    SAMPLE,<idx>,<irq_lat>,<isr_dur>,<sched_lat>,<total>,<warmup 0|1>
    DONE

Usage:
    tools/analyze.py --port /dev/ttyACM0            # capture one cycle of batches, then analyse
    tools/analyze.py --port /dev/ttyACM0 --batches 3
    tools/analyze.py --file tools/sample.log        # analyse log file(s) only
    tools/analyze.py                                # re-analyse everything captured so far

Captured batches are stored in <out>/raw/, and every run rebuilds
<out>/summary.md, <out>/<chip>_<bin>_<profile>.csv and the plots from all of them.
Batches without a chip= field (older firmware) count as rp2350.

Needs only the standard library (+ pyserial for --port). matplotlib is optional.
"""

import argparse
import csv
import math
import sys
import time
from collections import OrderedDict
from datetime import datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
METRICS = ["irq_lat", "isr_dur", "sched_lat", "total"]
METRIC_PT = {
    "irq_lat": "Lat. interrupção",
    "isr_dur": "Duração ISR",
    "sched_lat": "Lat. escalonamento",
    "total": "Total",
}
# chip -> (display name, core, default clk_sys in Hz)
CHIPS = {
    "rp2350": ("RP2350", "Cortex-M33", 150_000_000),
    "rp2040": ("RP2040", "Cortex-M0+", 125_000_000),
}
DEFAULT_CHIP = "rp2350"


def chip_name(chip):
    return CHIPS.get(chip, (chip.upper(),))[0]


def clk_tag(chip, clk):
    """File-name suffix for a non-default clock ('' at the chip's default clock)."""
    default = CHIPS.get(chip, (None, None, 0))[2]
    return "" if clk == default else f"_{clk // 1_000_000}mhz"


EXEC_PT = {
    "thread": "Embassy, executor de thread",
    "irq": "Embassy, InterruptExecutor",
    "bare": "Sem executor (cortex-m-rt)",
    "rtic": "RTIC 2",
    "zephyr_preempt": "Zephyr, threads preemptivas",
    "zephyr_coop": "Zephyr, threads cooperativas",
}
# Executors whose binary covers the idle case and the loads (L = 0 is part of Table B).
IDLE_AND_LOAD_EXECS = ("rtic", "zephyr_preempt", "zephyr_coop")


# ----------------------------------------------------------------- parsing

class Batch:
    def __init__(self, meta):
        self.meta = meta
        self.rows = []  # (irq_lat, isr_dur, sched_lat, total, warmup) in PWM ticks

    def get(self, key, default=None):
        return self.meta.get(key, default)

    def geti(self, key, default=0):
        try:
            return int(self.meta.get(key, default))
        except ValueError:
            return default

    @property
    def synthetic(self):
        return self.meta.get("synthetic", "0") not in ("0", "")

    @property
    def key(self):
        """Batches with the same key are pooled."""
        return (
            self.get("chip", DEFAULT_CHIP),
            self.get("bin", "?"),
            self.get("profile", "?"),
            self.geti("clk_sys_hz"),
            self.geti("load_us"),
            self.geti("ntasks"),
        )


def parse_lines(lines, warn):
    """Yield complete batches (META ... DONE) from an iterable of text lines."""
    cur = None
    for raw in lines:
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        tag, _, rest = line.partition(",")
        if tag == "META":
            if cur is not None:
                warn("lote incompleto descartado (novo META antes de DONE)")
            meta = OrderedDict()
            for item in rest.split(","):
                k, eq, v = item.partition("=")
                if eq:
                    meta[k.strip()] = v.strip()
            cur = Batch(meta)
        elif tag == "SAMPLE" and cur is not None:
            f = rest.split(",")
            try:
                idx = int(f[0])
                vals = [int(x) for x in f[1:5]]
                if len(vals) != 4:
                    raise ValueError
            except (ValueError, IndexError):
                warn(f"linha SAMPLE inválida ignorada: {line!r}")
                continue
            if len(f) > 5:
                warm = f[5].replace("warmup=", "").strip() == "1"
            else:
                warm = idx < cur.geti("warmup", 50)
            cur.rows.append((*vals, warm))
        elif tag == "DONE" and cur is not None:
            n = cur.geti("n", -1)
            if n >= 0 and n != len(cur.rows):
                warn(f"lote com {len(cur.rows)} amostras, META dizia n={n}")
            if cur.rows:
                yield cur
            else:
                warn("lote sem amostras ignorado")
            cur = None
    if cur is not None:
        warn("lote incompleto no fim da entrada (sem DONE) descartado")


def batch_to_text(b):
    out = ["META," + ",".join(f"{k}={v}" for k, v in b.meta.items())]
    for i, r in enumerate(b.rows):
        out.append(f"SAMPLE,{i},{r[0]},{r[1]},{r[2]},{r[3]},{1 if r[4] else 0}")
    out.append("DONE")
    return "\n".join(out) + "\n"


def capture(port, baud, want, timeout, warn):
    """Read batches from the serial port.

    want > 0: exactly that many batches.
    want = 0: one full cycle, i.e. until a batch configuration repeats.
    """
    try:
        import serial  # pyserial
    except ImportError:
        sys.exit("pyserial não encontrado: pip install pyserial (ou apt install python3-serial)")

    def lines(ser, deadline):
        while time.monotonic() < deadline:
            raw = ser.readline()
            if raw:
                yield raw.decode("ascii", errors="replace")

    batches, seen = [], set()
    with serial.Serial(port, baud, timeout=1) as ser:
        print(f"lendo {port} (até {timeout:.0f} s; a placa só envia entre um lote e outro)...", file=sys.stderr)
        deadline = time.monotonic() + timeout
        for b in parse_lines(lines(ser, deadline), warn):
            if want == 0 and b.key in seen:
                break
            seen.add(b.key)
            batches.append(b)
            print(
                f"  lote: {b.get('chip', DEFAULT_CHIP)} {b.get('bin')} {b.get('profile')} load_us={b.geti('load_us')} "
                f"ntasks={b.geti('ntasks')} n={len(b.rows)} descartadas={b.geti('discarded')}",
                file=sys.stderr,
            )
            if want and len(batches) >= want:
                break
    if not batches:
        sys.exit("nenhum lote completo recebido (a placa está rodando um binário de medição?)")
    return batches


# ------------------------------------------------------------------- stats

def percentile(sorted_vals, q):
    """Linear-interpolated percentile, q in [0, 100]."""
    if len(sorted_vals) == 1:
        return sorted_vals[0]
    pos = (len(sorted_vals) - 1) * q / 100.0
    lo = int(math.floor(pos))
    hi = min(lo + 1, len(sorted_vals) - 1)
    return sorted_vals[lo] + (sorted_vals[hi] - sorted_vals[lo]) * (pos - lo)


def stats(vals):
    """n, min, max, mean, sample std, p50, p99 and half-width of the 95 % CI of the mean."""
    n = len(vals)
    if n == 0:
        return None
    s = sorted(vals)
    mean = sum(s) / n
    std = math.sqrt(sum((v - mean) ** 2 for v in s) / (n - 1)) if n > 1 else 0.0
    return {
        "n": n,
        "min": s[0],
        "max": s[-1],
        "mean": mean,
        "std": std,
        "p50": percentile(s, 50),
        "p99": percentile(s, 99),
        "ci95": 1.96 * std / math.sqrt(n) if n > 1 else 0.0,
    }


class Group:
    """All batches of one configuration, converted to clk_sys cycles."""

    def __init__(self, key, batches):
        self.chip, self.bin, self.profile, self.clk, self.load_us, self.ntasks = key
        self.batches = batches
        first = batches[0]
        self.exec = first.get("exec", "?")
        self.opt = first.get("opt", "z" if self.profile.endswith("-z") else "3")
        self.git = first.get("git", "?")
        self.synthetic = any(b.synthetic for b in batches)
        self.discarded = sum(b.geti("discarded") for b in batches)
        self.divs = sorted({b.geti("pwm_div", 1) for b in batches})
        self.period_us = (first.geti("pwm_top") + 1) * first.geti("pwm_div", 1) / self.clk * 1e6 if self.clk else 0
        self.cycles = {m: {"all": [], "steady": []} for m in METRICS}
        for b in batches:
            div = b.geti("pwm_div", 1)
            for r in b.rows:
                for i, m in enumerate(METRICS):
                    c = r[i] * div
                    self.cycles[m]["all"].append(c)
                    if not r[4]:
                        self.cycles[m]["steady"].append(c)

    def us(self, cycles):
        return cycles / self.clk * 1e6

    def st(self, metric, which="steady"):
        """Statistics in cycles. which: 'all' (with warm-up) or 'steady' (without)."""
        return stats(self.cycles[metric][which])

    @property
    def clk_tag(self):
        return clk_tag(self.chip, self.clk)

    @property
    def file_tag(self):
        """<chip>_<bin>_<profile>[_<clk>mhz]: one file per chip, binary, profile and clock."""
        return f"{self.chip}_{self.bin}_{self.profile}{self.clk_tag}"

    @property
    def default_clk(self):
        return self.clk_tag == ""

    @property
    def label(self):
        s = self.bin
        if self.load_us:
            s += f" L={self.load_us}µs"
        if self.ntasks:
            s += f" N={self.ntasks}"
        return s


def build_groups(batches):
    pooled = OrderedDict()
    for b in batches:
        pooled.setdefault(b.key, []).append(b)
    groups = [Group(k, v) for k, v in pooled.items()]
    groups.sort(key=lambda g: (g.chip != DEFAULT_CHIP, g.chip, g.clk, g.profile, g.bin, g.load_us, g.ntasks))
    return groups


# ----------------------------------------------------------------- outputs

def load_paper():
    lat, mem, clk = {}, {}, 64_000_000
    path = ROOT / "reference" / "paper_results.csv"
    with open(path, newline="") as f:
        for row in csv.DictReader(l for l in f if not l.startswith("#")):
            if row["table"] == "meta" and row["metric"] == "clk_hz":
                clk = int(row["value"])
            elif row["table"] == "latency":
                lat[(row["system"], row["opt"], row["metric"])] = float(row["value"])
            elif row["table"] == "memory":
                mem[(row["system"], row["metric"])] = int(row["value"])
    return lat, mem, clk


def write_csvs(groups, out):
    files = OrderedDict()
    for g in groups:
        files.setdefault(out / f"{g.file_tag}.csv", []).append(g)
    for path, gs in files.items():
        with open(path, "w", newline="") as f:
            w = csv.writer(f)
            w.writerow(
                ["chip", "bin", "profile", "exec", "clk_sys_hz", "load_us", "ntasks", "batches", "discarded", "synthetic",
                 "metric", "warmup_included", "n",
                 "min_us", "max_us", "mean_us", "std_us", "p50_us", "p99_us", "ci95_us",
                 "min_cycles", "max_cycles", "mean_cycles", "std_cycles", "p50_cycles", "p99_cycles", "ci95_cycles"]
            )
            for g in gs:
                for m in METRICS:
                    for which in ("all", "steady"):
                        s = g.st(m, which)
                        if not s:
                            continue
                        order = ["min", "max", "mean", "std", "p50", "p99", "ci95"]
                        w.writerow(
                            [g.chip, g.bin, g.profile, g.exec, g.clk, g.load_us, g.ntasks, len(g.batches), g.discarded,
                             int(g.synthetic), m, int(which == "all"), s["n"]]
                            + [f"{g.us(s[k]):.4f}" for k in order]
                            + [f"{s[k]:.2f}" for k in order]
                        )
    return list(files)


def fmt_us(v):
    return f"{v:.3f}" if v < 100 else f"{v:.1f}"


def table(header, rows):
    out = ["| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    return "\n".join(out) + "\n"


def read_sizes(out):
    path = out / "sizes.csv"
    if not path.exists():
        return None
    with open(path, newline="") as f:
        rows = list(csv.DictReader(f))
    for r in rows:
        r.setdefault("chip", DEFAULT_CHIP)  # files written before the chip column existed
        r["chip"] = r["chip"] or DEFAULT_CHIP
    return rows


def write_summary(groups, out, warnings):
    paper_lat, paper_mem, paper_clk = load_paper()
    L = []
    add = L.append
    synthetic = any(g.synthetic for g in groups)

    chips = sorted({g.chip for g in groups}, key=lambda c: (c != DEFAULT_CHIP, c))
    add(f"# Resumo das medições — {' / '.join(chip_name(c) for c in chips)} × artigo (Sensors 2024, 24, 5818)\n")
    add(f"Gerado por `tools/analyze.py` em {datetime.now():%Y-%m-%d %H:%M}.\n")
    if synthetic:
        add("> **ATENÇÃO: DADOS SINTÉTICOS.** Pelo menos um lote veio de um log de exemplo "
            "(`synthetic=1`). Estes números **não são medições** e não podem ser usados no seminário.\n")
    add("Latências medidas com o contador do PWM (tempo desde o evento). "
        "“Sem aquecimento” exclui as amostras marcadas `warmup=1` (as 50 primeiras de cada lote); "
        "“com aquecimento” inclui todas. Médias são dadas sem aquecimento; **máximos, com aquecimento**, "
        "porque as falhas iniciais de cache fazem parte do pior caso real.\n")

    sections = sorted({(g.chip, g.clk) for g in groups}, key=lambda k: (k[0] != DEFAULT_CHIP, k[0], k[1]))
    for chip, clk in sections:
        gs = [g for g in groups if g.chip == chip and g.clk == clk]
        cname, core = chip_name(chip), CHIPS.get(chip, ("", "?"))[1]
        add(f"## {cname} ({core}) — clk_sys = {clk / 1e6:g} MHz\n")

        # ---- Table A
        add("### Tabela A — sistema ocioso (S0): nossas médias × artigo\n")
        add(f"Artigo: nRF52840 (Cortex-M4) a {paper_clk / 1e6:g} MHz, média com sistema ocioso. "
            f"Ciclos = µs × clock; a comparação em ciclos é só indicativa ({core} × Cortex-M4, cache XIP × flash interna).\n")
        idle = [g for g in gs if g.load_us == 0 and g.ntasks == 0]
        if not idle:
            add("_Sem dados de S0 ainda (grave `s0_thread_idle` e `s0_irq_idle`)._\n")
        for opt in ("3", "z"):
            rows = []
            for g in [g for g in idle if g.opt == opt]:
                cells = [f"{cname} — {EXEC_PT.get(g.exec, g.exec)} (`{g.bin}`)"]
                for m in METRICS:
                    s = g.st(m)
                    cells.append(f"{fmt_us(g.us(s['mean']))} µs ({s['mean']:.0f} cic.)")
                smax = g.st("total", "all")
                cells.append(f"{fmt_us(g.us(smax['max']))} µs")
                rows.append(cells)
            if not rows:
                continue
            for system in ("Embassy", "RTIC"):
                cells = [f"Artigo — {system}"]
                for m in METRICS:
                    v = paper_lat.get((system, opt, m))
                    cells.append(f"{v:g} µs ({v * paper_clk / 1e6:.0f} cic.)" if v is not None else "—")
                cells.append("não informado")
                rows.append(cells)
            add(f"**opt-level = {'3' if opt == '3' else '\"z\"'}**\n")
            add(table(["Sistema"] + [f"{METRIC_PT[m]} (média)" for m in METRICS] + ["Total (máx.)"], rows))

        # ---- Table B
        add("### Tabela B — com carga (S1): média × máximo da latência total\n")
        add("Tabela central do seminário. `L` é o trecho de CPU sem `.await` da tarefa de carga. "
            "Hipótese inicial: no executor de thread o máximo cresce até ≈ L; com `InterruptExecutor` ele fica perto do caso ocioso. "
            "No Embassy 0.10 o executor de thread chega a ≈ 2L, porque a fila de prontos é uma pilha (README, seção 7.2).\n")
        load = [g for g in gs if g.ntasks == 0 and g.exec in ("thread", "irq") + IDLE_AND_LOAD_EXECS
                and (g.load_us > 0 or g.bin.startswith("s0_") or g.exec in IDLE_AND_LOAD_EXECS)]
        if not any(g.load_us for g in load):
            add("_Sem dados de S1 ainda (grave `s1_thread_load` e `s1_irq_load`)._\n")
        else:
            for profile in sorted({g.profile for g in load}):
                rows = []
                for g in sorted([g for g in load if g.profile == profile], key=lambda g: (g.exec != "thread", g.exec, g.load_us)):
                    s_mean, s_all, s_steady = g.st("total"), g.st("total", "all"), g.st("total", "steady")
                    ratio = f"{g.us(s_all['max']) / g.load_us:.2f}" if g.load_us else "—"
                    rows.append([
                        EXEC_PT.get(g.exec, g.exec),
                        g.load_us if g.load_us else "0 (ocioso)",
                        s_all["n"],
                        g.discarded,
                        fmt_us(g.us(s_mean["mean"])),
                        fmt_us(g.us(s_mean["p99"])),
                        f"**{fmt_us(g.us(s_all['max']))}**",
                        fmt_us(g.us(s_steady["max"])),
                        ratio,
                    ])
                add(f"**perfil `{profile}`** (valores em µs)\n")
                add(table(["Executor da tarefa medida", "L (µs)", "n", "descartadas", "média", "p99",
                           "máximo", "máx. sem aquec.", "máx. / L"], rows))

        # ---- S2
        nt = [g for g in gs if g.ntasks > 0]
        if nt:
            add("### Latência × número de tarefas extras (S2, Figura 4 do artigo)\n")
            add("Cada tarefa extra faz `Timer::after_micros(k).await` em loop, com `k` diferente por tarefa "
                "(o artigo não diz o que as tarefas dele faziam).\n")
            for profile in sorted({g.profile for g in nt}):
                rows = []
                for g in sorted([g for g in nt if g.profile == profile], key=lambda g: (g.bin, g.ntasks)):
                    s, s_all = g.st("total"), g.st("total", "all")
                    rows.append([f"`{g.bin}`", g.ntasks, s_all["n"], g.discarded, fmt_us(g.us(s["mean"])),
                                 fmt_us(g.us(s["p99"])), fmt_us(g.us(s_all["max"]))])
                add(f"**perfil `{profile}`** (latência total, µs)\n")
                add(table(["Binário", "N tarefas extras", "n", "descartadas", "média", "p99", "máximo"], rows))

    # ---- Chip comparison
    if len(chips) > 1:
        write_chip_comparison(groups, chips, add)

    # ---- Table C
    add("## Tabela C — memória de um blink (bytes)\n")
    sizes = read_sizes(out)
    rows = []
    if sizes:
        for r in sorted(sizes, key=lambda r: (r["chip"] != DEFAULT_CHIP, r["chip"])):
            rows.append([f"{chip_name(r['chip'])} — `{r['bin']}` (`{r['profile']}`)", r["text"], r["data"], r["bss"], r["total"]])
    for system in ("Embassy", "RTIC"):
        rows.append([f"Artigo — {system} (opt-level \"z\")"] + [paper_mem.get((system, k), "—") for k in ("text", "data", "bss", "total")])
    add(table(["Sistema", ".text", ".data", ".bss", "Total"], rows))
    if sizes:
        add("Nossas colunas são as do `size` no formato Berkeley: `text` soma código, constantes (`.rodata`), "
            "tabela de vetores e o bloco de boot (`IMAGE_DEF` no RP2350; `.boot2`, 256 bytes, no RP2040).\n")
    else:
        add("_Nossas linhas ainda não existem: rode `tools/sizes.sh`._\n")

    # ---- Full statistics
    add("## Estatísticas completas\n")
    add("Valores em µs; entre colchetes, em ciclos de clk_sys. IC 95 % = meia-largura do intervalo de confiança da média.\n")
    for g in groups:
        add(f"### `{g.bin}` — {chip_name(g.chip)}, `{g.profile}`, {g.clk / 1e6:g} MHz"
            + (f", L = {g.load_us} µs" if g.load_us else "")
            + (f", N = {g.ntasks}" if g.ntasks else "")
            + (" — SINTÉTICO" if g.synthetic else "") + "\n")
        res = ", ".join(f"{d} ciclo(s) ({d / g.clk * 1e9:.1f} ns)" for d in g.divs)
        add(f"{len(g.batches)} lote(s), período do evento {g.period_us:.1f} µs, resolução {res}, "
            f"{g.discarded} evento(s) descartado(s), git `{g.git}`.\n")
        rows = []
        for m in METRICS:
            for which, name in (("steady", "sem"), ("all", "com")):
                s = g.st(m, which)
                if not s:
                    continue
                rows.append([METRIC_PT[m], name, s["n"],
                             fmt_us(g.us(s["min"])), fmt_us(g.us(s["mean"])), "± " + fmt_us(g.us(s["ci95"])),
                             fmt_us(g.us(s["std"])), fmt_us(g.us(s["p50"])), fmt_us(g.us(s["p99"])),
                             f"{fmt_us(g.us(s['max']))} [{s['max']:.0f}]"])
        add(table(["Métrica", "Aquec.", "n", "mín.", "média", "IC 95 %", "desvio", "p50", "p99", "máx."], rows))

    if warnings:
        add("## Avisos\n")
        add("\n".join(f"- {w}" for w in warnings) + "\n")

    path = out / "summary.md"
    path.write_text("\n".join(L))
    return path


def write_chip_comparison(groups, chips, add):
    """Same scenario on both chips, each at its default clock (or both at the same
    non-default clock, e.g. 64 MHz): mean and maximum of the total latency."""
    a, b = chips[0], chips[1]
    add(f"## Comparação {chip_name(a)} × {chip_name(b)} — latência total\n")
    add("Mesmo binário, perfil, `L` e `N` nos dois chips. Cada chip no seu clock padrão "
        f"({', '.join(f'{chip_name(c)} {CHIPS[c][2] / 1e6:g} MHz' for c in (a, b) if c in CHIPS)}), "
        "ou os dois no mesmo clock reduzido. Em µs a diferença mistura clock e núcleo; "
        "em ciclos sobra o efeito do núcleo, da memória (XIP) e do código gerado. "
        "Média sem aquecimento; máximo com aquecimento. "
        "No S1 com executor de thread a latência é dominada por `L`, que é tempo de relógio: "
        "lá compare os µs, não a razão em ciclos.\n")
    def cfg(g):
        return (g.bin, g.profile, g.load_us, g.ntasks, "padrão" if g.default_clk else f"{g.clk / 1e6:g} MHz")
    by = {(g.chip, cfg(g)): g for g in groups}
    keys = sorted({cfg(g) for g in groups if (a, cfg(g)) in by and (b, cfg(g)) in by},
                  key=lambda k: (k[4] != "padrão", k[4], k[1], k[0], k[2], k[3]))
    if not keys:
        add("_Nenhum cenário medido nos dois chips ainda._\n")
        return
    rows = []
    for k in keys:
        ga, gb = by[(a, k)], by[(b, k)]
        sa_m, sb_m = ga.st("total"), gb.st("total")
        sa_x, sb_x = ga.st("total", "all"), gb.st("total", "all")
        rows.append([
            ga.label, f"`{k[1]}`", k[4],
            f"{fmt_us(ga.us(sa_m['mean']))} / {fmt_us(gb.us(sb_m['mean']))}",
            f"{fmt_us(ga.us(sa_x['max']))} / {fmt_us(gb.us(sb_x['max']))}",
            f"{sa_m['mean']:.0f} / {sb_m['mean']:.0f}",
            f"{sb_m['mean'] / sa_m['mean']:.2f}" if sa_m["mean"] else "—",
        ])
    na, nb = chip_name(a), chip_name(b)
    add(table(["Cenário", "Perfil", "Clock", f"Média µs ({na} / {nb})", f"Máx. µs ({na} / {nb})",
               f"Média ciclos ({na} / {nb})", f"Ciclos {nb} ÷ {na}"], rows))


def write_plots(groups, out):
    try:
        import matplotlib
        matplotlib.use("Agg")
        import matplotlib.pyplot as plt
    except ImportError:
        print("matplotlib não encontrado: gráficos não gerados", file=sys.stderr)
        return []
    made = []
    synthetic = any(g.synthetic for g in groups)
    mark = " — DADOS SINTÉTICOS" if synthetic else ""

    # Mean and maximum of the total latency per scenario.
    for chip, clk in sorted({(g.chip, g.clk) for g in groups}):
        for profile in sorted({g.profile for g in groups if g.chip == chip and g.clk == clk}):
            gs = [g for g in groups if g.chip == chip and g.clk == clk and g.profile == profile]
            labels = [g.label for g in gs]
            means = [g.us(g.st("total")["mean"]) for g in gs]
            maxes = [g.us(g.st("total", "all")["max"]) for g in gs]
            fig, ax = plt.subplots(figsize=(max(7, 1.1 * len(gs) + 3), 4.8))
            xs = range(len(gs))
            ax.bar([x - 0.2 for x in xs], means, 0.4, label="média", color="#2a78d6")
            ax.bar([x + 0.2 for x in xs], maxes, 0.4, label="máximo", color="#d6452a")
            for x, v in zip(xs, maxes):
                ax.text(x + 0.2, v, f"{v:.1f}", ha="center", va="bottom", fontsize=8)
            for x, v in zip(xs, means):
                ax.text(x - 0.2, v, f"{v:.1f}", ha="center", va="bottom", fontsize=8)
            ax.set_yscale("log")
            ax.set_xticks(list(xs))
            ax.set_xticklabels(labels, rotation=30, ha="right")
            ax.set_ylabel("latência total (µs, escala log)")
            ax.set_title(f"Latência total por cenário — {chip_name(chip)}, {profile}, {clk / 1e6:g} MHz{mark}")
            ax.legend()
            ax.grid(axis="y", which="both", alpha=0.3)
            fig.tight_layout()
            main = gs[0].default_clk and profile == "release"
            name = f"latency_{chip}.png" if main else f"latency_{chip}_{profile}{gs[0].clk_tag}.png"
            fig.savefig(out / name, dpi=150)
            plt.close(fig)
            made.append(out / name)

    # Histogram of the total latency per scenario.
    for g in groups:
        vals = [g.us(c) for c in g.cycles["total"]["all"]]
        fig, ax = plt.subplots(figsize=(7, 4))
        ax.hist(vals, bins=80, color="#2a78d6")
        ax.set_yscale("log")
        ax.set_xlabel("latência total (µs)")
        ax.set_ylabel("eventos (escala log)")
        ax.set_title(f"{g.label} — {chip_name(g.chip)}, {g.profile}, {g.clk / 1e6:g} MHz{mark}", fontsize=10)
        ax.axvline(max(vals), color="#d6452a", linestyle="--", linewidth=1, label=f"máx. = {max(vals):.2f} µs")
        ax.legend()
        fig.tight_layout()
        suffix = (f"_L{g.load_us}" if g.load_us else "") + (f"_N{g.ntasks}" if g.ntasks else "")
        name = f"hist_{g.file_tag}{suffix}.png"
        fig.savefig(out / name, dpi=120)
        plt.close(fig)
        made.append(out / name)
    return made


# -------------------------------------------------------------------- main

def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", help="porta serial da placa, ex.: /dev/ttyACM0")
    ap.add_argument("--baud", type=int, default=115200, help="irrelevante para USB CDC; não use 1200 (reinicia em BOOTSEL)")
    ap.add_argument("--file", nargs="+", help="analisa só este(s) arquivo(s) de log, sem capturar")
    ap.add_argument("--batches", type=int, default=0,
                    help="lotes a capturar; padrão 0 = um ciclo completo (até a configuração se repetir)")
    ap.add_argument("--timeout", type=float, default=180, help="tempo máximo de captura, em segundos")
    ap.add_argument("--append", action="store_true", help="acrescenta aos lotes já capturados deste binário em vez de substituir")
    ap.add_argument("--out", help="diretório de saída (padrão: results/)")
    args = ap.parse_args()
    if args.port and args.file:
        ap.error("use --port ou --file, não os dois")

    warnings = []

    def warn(msg):
        warnings.append(msg)
        print(f"aviso: {msg}", file=sys.stderr)

    out = Path(args.out) if args.out else ROOT / "results"

    if args.file:
        batches = []
        for name in args.file:
            with open(name) as f:
                batches += list(parse_lines(f, warn))
        # Never mix made-up numbers with real results.
        if any(b.synthetic for b in batches) and not args.out:
            out = ROOT / "results-synthetic"
            warn(f"log sintético: saída redirecionada para {out.name}/ (results/ é só para medições reais)")
        out.mkdir(parents=True, exist_ok=True)
    else:
        out.mkdir(parents=True, exist_ok=True)
        raw = out / "raw"
        raw.mkdir(exist_ok=True)
        if args.port:
            new = capture(args.port, args.baud, args.batches, args.timeout, warn)
            per_file = OrderedDict()
            for b in new:
                chip = b.get("chip", DEFAULT_CHIP)
                tag = clk_tag(chip, b.geti("clk_sys_hz"))
                name = f"{chip}_{b.get('bin', 'unknown')}_{b.get('profile', 'unknown')}{tag}.log"
                per_file.setdefault(raw / name, []).append(b)
            for path, bs in per_file.items():
                with open(path, "a" if args.append else "w") as f:
                    f.write("".join(batch_to_text(b) for b in bs))
                print(f"salvo: {path}", file=sys.stderr)
        batches = []
        for path in sorted(raw.glob("*.log")):
            with open(path) as f:
                batches += list(parse_lines(f, warn))

    if not batches:
        sys.exit("nenhum lote para analisar (use --port ou --file)")

    groups = build_groups(batches)
    for g in groups:
        if g.discarded:
            warn(f"{g.label} ({g.profile}): {g.discarded} evento(s) descartado(s) — latência próxima ou acima do período do evento")
    made = write_csvs(groups, out)
    made.append(write_summary(groups, out, warnings))
    made += write_plots(groups, out)

    print(f"\n{len(batches)} lote(s), {len(groups)} configuração(ões). Arquivos em {out}/:")
    for p in made:
        print(f"  {p.name}")
    print()
    for g in groups:
        s, s_all = g.st("total"), g.st("total", "all")
        print(f"  {g.chip:6s} {g.label:32s} {g.profile:10s} total: média {g.us(s['mean']):9.3f} µs   máx {g.us(s_all['max']):9.3f} µs"
              + ("   [SINTÉTICO]" if g.synthetic else ""))


if __name__ == "__main__":
    main()
