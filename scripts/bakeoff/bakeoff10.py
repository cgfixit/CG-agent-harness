#!/usr/bin/env python3
"""bakeoff10: cg-researcher private-corpus -> HTML research-infographic bakeoff.
Wire: Ollama /v1/chat/completions, temp 0.2, no reasoning_effort; strip think/channel leaks."""
import argparse,json,re,time,pathlib,urllib.request,subprocess
H=pathlib.Path(__file__).parent
PROMPT="""I've been dumping notes about our neighborhood tool library into a folder. Here's everything in it:

{docs}

Can you dig through these and tell me what's actually going on — what's growing, what people want, and what's worth paying attention to next? Only go off what's in the files; if something I'd want to know isn't in there, just say it's not covered. Tag each claim with the file id in brackets like [D1].

Deliver it as ONE self-contained HTML page (inline CSS, no external links/scripts) that works as a little research infographic: headline findings, a few simple visual bars or stat cards using the real numbers, a "trends" section, and an "open questions / not in the notes" section. Output only the HTML."""
def docs():
    return "\n\n".join(f"=== [{p.stem.split('_')[0]}] {p.name} ===\n{p.read_text()}" for p in sorted((H/'corpus').iterdir()) if p.is_file())
def strip(t):
    t=re.sub(r"<think>.*?</think>","",t,flags=re.S); t=re.sub(r"<\|channel\|>.*?<\|message\|>","",t,flags=re.S)
    m=re.search(r"<!DOCTYPE html.*?</html>|<html.*?</html>",t,flags=re.S|re.I); return t,(m.group(0) if m else "")
FACT_PATS={
 "412 members":[r"412"],"298 prior":[r"298"],"Fenwick 3150":[r"3,?150"],"Lena Ortiz":[r"Lena Ortiz"],
 "vote 5-2":[r"5-2|5–2"],"n=87":[r"\b87\b"],"54/87":[r"54"],"sewing 29":[r"\b29\b"],
 "weekday hours":[r"weekday|weekend"],"05-02 14/9":[r"14"],"06-06 22/17":[r"17"],
 "fixers 3->6":[r"\b3\b.*\b6\b|3 to 6|3→6"],"07-04 cancelled":[r"cancel"],
 "Priya Nand":[r"Priya"],"no budget/decision":[r"no budget|no decision|not (yet )?approved"],
 "Makita 61":[r"61"],"cite D1":[r"\[D1\]"],"cite D2":[r"\[D2\]"],"cite D3":[r"\[D3\]"],"cite D4":[r"\[D4\]"],
}
TRAP_PATS={
 "income/revenue/fee mentions (check context)":[r"income|demographic|revenue|dues|membership fee"],
 "pilot stated approved/funded":[r"pilot (was |has been )?(approved|funded|greenlit)"],
 "July repair numbers":[r"July[^<]{0,40}(fixed|brought|\d+\s*/\s*\d+)"],
 "grant not 3,150":[r"\$\d{1,3}(?:,\d{3})+(?!.*3,?150)"],
}
CITE=re.compile(r"\[D[1-4]\]")
def score_dir(d):
    gold=json.loads((H/"gold.json").read_text()); tot=len(FACT_PATS)
    for f in sorted(pathlib.Path(d).glob("*.html")):
        html=f.read_text(); txt=re.sub(r"<[^>]+>"," ",html)
        miss=[k for k,pats in FACT_PATS.items() if not any(re.search(p,html,re.I|re.S) or re.search(p,txt,re.I) for p in pats)]
        traps={k:sum(len(re.findall(p,txt,re.I)) for p in pats) for k,pats in TRAP_PATS.items()}
        fmt={"has_html":bool(re.search(r"<html",html,re.I)),"ext_link":bool(re.search(r"https?://",html)),"inline_css":"<style" in html.lower()}
        n=tot-len(miss); print(f"{f.stem}: facts {n}/{tot} cites {len(CITE.findall(txt))} fmt {fmt}\n  missed {miss}\n  trap-hits {traps}")
ap=argparse.ArgumentParser()
ap.add_argument("--models",default="gemma4-26b-a4b:ud-q4_k_xl-cg,qwen3.8:27b-mlx-cg,qwen3.6:35b-a3b-cg")
ap.add_argument("--out",default=str(H/"out")); ap.add_argument("--dry-run",action="store_true")
ap.add_argument("--quick",action="store_true",help="first model only, max_tokens 4000")
ap.add_argument("--max-tokens",type=int,default=8000); ap.add_argument("--score",metavar="DIR")
a=ap.parse_args()
if a.score: score_dir(a.score); raise SystemExit
models=a.models.split(","); mt=4000 if a.quick else a.max_tokens
if a.quick: models=models[:1]
prompt=PROMPT.format(docs=docs()); out=pathlib.Path(a.out); out.mkdir(parents=True,exist_ok=True)
if a.dry_run:
    print(prompt); print(f"\n--- plan: {len(prompt)} chars, max_tokens {mt}, out {out}")
    for lab,m in zip("ABCDEFGH",models): print(f"  {lab} = {m}")
    raise SystemExit
for lab,m in zip("ABCDEFGH",models):
    print(f"starting {lab}={m} ...", flush=True)
    body={"model":m,"messages":[{"role":"user","content":prompt}],"temperature":0.2,"max_tokens":mt}
    t=time.time()
    try:
        r=json.load(urllib.request.urlopen(urllib.request.Request("http://localhost:11434/v1/chat/completions",json.dumps(body).encode(),{"Content-Type":"application/json"}),timeout=1800)); err=None
    except Exception as e: r={}; err=repr(e)
    w=round(time.time()-t,1); c=(r.get("choices") or [{}])[0].get("message",{}).get("content","") or ""
    full,html=strip(c); (out/f"{lab}.raw.json").write_text(json.dumps(r,indent=1)); (out/f"{lab}.html").write_text(html)
    ps=subprocess.run(["ollama","ps"],capture_output=True,text=True).stdout
    meta={"label":lab,"model":m,"wall_s":w,"usage":r.get("usage"),"finish":(r.get("choices") or [{}])[0].get("finish_reason"),"think_leak":"<think>" in c,"html_len":len(html),"err":err,"ollama_ps":ps}
    (out/f"{lab}.meta.json").write_text(json.dumps(meta,indent=1)); print(json.dumps({k:meta[k] for k in meta if k!="ollama_ps"}), flush=True); print(ps, flush=True)
    subprocess.run(["ollama","stop",m])
score_dir(out)
