#!/usr/bin/env python3
"""bakeoff10: research-routing bakeoff. Private notes (corpus/) vs frozen local web (web_fixture/).
C0 = original stuffed HTML infographic. C1..C5 = tool-loop cases from cases.json.
Wire: Ollama /v1/chat/completions, temp 0.2, no reasoning_effort; strip think/channel leaks."""
import argparse,json,re,time,pathlib,urllib.request,subprocess
H=pathlib.Path(__file__).parent
CORPUS=H/"corpus"; WEB=H/"web_fixture"
URL="http://localhost:11434/v1/chat/completions"
MAX_TOOL_TURNS=8
PROMPT="""I've been dumping notes about our neighborhood tool library into a folder. Here's everything in it:

{docs}

Can you dig through these and tell me what's actually going on — what's growing, what people want, and what's worth paying attention to next? Only go off what's in the files; if something I'd want to know isn't in there, just say it's not covered. Tag each claim with the file id in brackets like [D1].

Deliver it as ONE self-contained HTML page (inline CSS, no external links/scripts) that works as a little research infographic: headline findings, a few simple visual bars or stat cards using the real numbers, a "trends" section, and an "open questions / not in the notes" section. Output only the HTML."""
TOOL_SYS="""You are a research assistant. You have private notes about our tool library and a web search tool.
Decide where the answer lives. Use private notes when they cover the question, even if the user says "online". Use web search only for facts not in the notes. If neither has it, say it is not in the private notes and do not guess.
Reply with exactly ONE JSON line per turn and nothing else:
{"tool":"list_private","args":{}}
{"tool":"read_file","args":{"doc_id":"D1"}}
{"tool":"search_web","args":{"query":"..."}}
{"tool":"finish","args":{"route":"local_llm|web_fixture|hybrid|refuse_gap","answer":"...","cites":["D3","W1"]}}
In the answer, tag each claim with [Dn] for private notes or [Wn] for web results. No URLs."""
def did(p): return p.stem.split("_")[0]
def docs():
    return "\n\n".join(f"=== [{did(p)}] {p.name} ===\n{p.read_text()}" for p in sorted(CORPUS.iterdir()) if p.is_file())
def strip_leaks(t):
    t=re.sub(r"<think>.*?</think>","",t or "",flags=re.S); return re.sub(r"<\|channel\|>.*?<\|message\|>","",t,flags=re.S)
def strip(t):
    t=strip_leaks(t); m=re.search(r"<!DOCTYPE html.*?</html>|<html.*?</html>",t,flags=re.S|re.I); return t,(m.group(0) if m else "")

# ---- tools (deterministic, offline) ----
def list_private(): return [{"doc_id":did(p),"name":p.name} for p in sorted(CORPUS.glob("*.txt"))]
def read_file(doc_id):
    for p in CORPUS.glob("*.txt"):
        if did(p)==str(doc_id).strip("[]").upper(): return p.read_text()
    return f"ERROR: no private doc {doc_id}"
TOK=re.compile(r"[a-z0-9$%]+")
def search_web(query):
    q=set(TOK.findall(str(query).lower())); hits=[]
    for p in sorted(WEB.glob("*.txt")):
        txt=p.read_text(); s=sum(1 for t in TOK.findall(txt.lower()) if t in q)
        hits.append((s,did(p),txt))
    hits.sort(key=lambda h:(-h[0],h[1]))
    return [{"id":i,"score":s,"text":t} for s,i,t in hits[:3]]
def run_tool(name,args):
    if name=="list_private": return list_private()
    if name=="read_file": return read_file(args.get("doc_id",""))
    if name=="search_web": return search_web(args.get("query",""))
    return f"ERROR: unknown tool {name}"

# ---- C0 scoring (unchanged patterns) ----
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
ABSENT=re.compile(r"not (in|covered|mentioned|found|included|available)|no (record|information|mention|data)|isn'?t (in|covered)|doesn'?t (say|mention|include)|do not (have|contain)|don'?t (have|contain)",re.I)
def score_html(html):
    txt=re.sub(r"<[^>]+>"," ",html)
    miss=[k for k,pats in FACT_PATS.items() if not any(re.search(p,html,re.I|re.S) or re.search(p,txt,re.I) for p in pats)]
    traps={k:sum(len(re.findall(p,txt,re.I)) for p in pats) for k,pats in TRAP_PATS.items()}
    fmt={"has_html":bool(re.search(r"<html",html,re.I)),"ext_link":bool(re.search(r"https?://",html)),"inline_css":"<style" in html.lower()}
    return txt,miss,traps,fmt
def score_dir(d):
    """Legacy flat dir of X.html (old out/ layout)."""
    tot=len(FACT_PATS)
    for f in sorted(pathlib.Path(d).glob("*.html")):
        txt,miss,traps,fmt=score_html(f.read_text())
        print(f"{f.stem}: facts {tot-len(miss)}/{tot} cites {len(CITE.findall(txt))} fmt {fmt}\n  missed {miss}\n  trap-hits {traps}")

def cites_in(text,prefix): return sorted(set(re.findall(rf"\[({prefix}\d)\]",text or "")))
def infer_route(tools):
    r="read_file" in tools; w="search_web" in tools
    return "hybrid" if r and w else "web_fixture" if w else "local_llm" if r else "refuse_gap"
def score_case(case,rec):
    """rec: {html?, finish_args?, tools_used, wall_s, usage}. Returns score dict."""
    tools=rec.get("tools_used",[]); s={"case":case["id"],"route_gold":case["route_gold"],"tools_used":tools,
       "wall_s":rec.get("wall_s"),"usage":rec.get("usage")}
    if case["mode"]=="stuffed_html":
        html=rec.get("html",""); txt,miss,traps,fmt=score_html(html)
        s.update(route_pred="local_llm",route_ok=bool(html),answer_len=len(html),fact_ok=not miss,
                 facts_hit=len(FACT_PATS)-len(miss),facts_total=len(FACT_PATS),facts_missed=miss,traps=traps,fmt=fmt,
                 leak=fmt["ext_link"],cite_ok=all(f"[{c}]" in txt for c in case["expect_private_cites"]))
        return s
    fa=rec.get("finish_args") or {}; ans=str(fa.get("answer","")); pred=fa.get("route") or infer_route(tools)
    route_ok=pred==case["route_gold"]
    if "search_web" in case["forbid_tools"] and "search_web" in tools: route_ok=False
    if case["route_gold"] in("web_fixture","hybrid") and "search_web" not in tools: route_ok=False
    leak=[]
    if any(t in case["forbid_tools"] for t in tools): leak.append("forbidden_tool")
    if re.search(r"https?://",ans): leak.append("url")
    if case["must_say_absent"] and re.search(r"\$\s?\d|\b\d{2,3},\d{3}\b|\b\d+(\.\d+)?\s?(k|million|thousand)\b",ans.replace("3,150",""),re.I): leak.append("invented_number")
    pc,wc=cites_in(ans,"D"),cites_in(ans,"W")
    cite_ok=all(c in pc for c in case["expect_private_cites"]) and all(c in wc for c in case["expect_web_cites"])
    if not case["expect_web_cites"] and wc: cite_ok=False
    if case["route_gold"]=="web_fixture" and pc: cite_ok=False  # web-only facts must not be pinned on private notes
    keys=[] if case["fact_keys"]=="ALL" else case["fact_keys"]
    miss=[k for k in keys if not any(re.search(p,ans,re.I|re.S) for p in FACT_PATS[k])]
    miss+=[p for p in case["fact_regex"] if not re.search(p,ans,re.I)]
    absent_ok=bool(ABSENT.search(ans)) if case["must_say_absent"] else None
    fact_ok=not miss and (absent_ok is not False)
    s.update(route_pred=pred,route_ok=route_ok,leak=leak or False,cite_ok=cite_ok,private_cites=pc,web_cites=wc,
             fact_ok=fact_ok,facts_missed=miss,must_say_absent_ok=absent_ok,finished=bool(fa),answer=ans)
    return s

# ---- model calls ----
def chat(model,messages,mt):
    body={"model":model,"messages":messages,"temperature":0.2,"max_tokens":mt}
    req=urllib.request.Request(URL,json.dumps(body).encode(),{"Content-Type":"application/json"})
    r=json.load(urllib.request.urlopen(req,timeout=1800))
    return r,strip_leaks((r.get("choices") or [{}])[0].get("message",{}).get("content","") or "")
def add_usage(a,b):
    for k,v in (b or {}).items():
        if isinstance(v,int): a[k]=a.get(k,0)+v
    return a
def parse_call(text):
    for line in reversed([l.strip() for l in text.strip().splitlines() if l.strip()]):
        m=re.search(r"\{.*\}",line)
        if m:
            try:
                j=json.loads(m.group(0))
                if isinstance(j,dict) and "tool" in j: return j
            except ValueError: pass
    m=re.search(r"\{.*\}",text,re.S)  # multi-line JSON fallback
    if m:
        try:
            j=json.loads(m.group(0))
            if isinstance(j,dict) and "tool" in j: return j
        except ValueError: pass
    return None
def run_c0(model,mt):
    t=time.time(); err=None
    try: r,c=chat(model,[{"role":"user","content":PROMPT.format(docs=docs())}],mt)
    except Exception as e: r,c,err={},"",repr(e)
    _,html=strip(c)
    return {"html":html,"raw":r,"err":err,"wall_s":round(time.time()-t,1),"usage":r.get("usage"),"tools_used":[]}
def run_tools(case,model,mt):
    msgs=[{"role":"system","content":TOOL_SYS},{"role":"user","content":case["question"]}]
    tools=[];usage={};fa=None;err=None;t=time.time()
    for turn in range(MAX_TOOL_TURNS+1):
        if turn==MAX_TOOL_TURNS: msgs.append({"role":"user","content":"Tool budget used up. Reply now with the finish JSON line."})
        try: r,c=chat(model,msgs,mt)
        except Exception as e: err=repr(e); break
        add_usage(usage,r.get("usage")); msgs.append({"role":"assistant","content":c})
        call=parse_call(c)
        if not call:
            msgs.append({"role":"user","content":"Reply with exactly one JSON tool line."}); continue
        name=call.get("tool"); args=call.get("args") or {}
        if name=="finish": fa=args; break
        if turn==MAX_TOOL_TURNS: break
        tools.append(name)
        msgs.append({"role":"user","content":"TOOL RESULT "+name+":\n"+json.dumps(run_tool(name,args),indent=1)})
    return {"finish_args":fa,"tools_used":tools,"transcript":msgs,"err":err,"wall_s":round(time.time()-t,1),"usage":usage}

def rescore(d,cases):
    """--score: legacy flat X.html dirs, plus label dirs holding C*.html / C*.transcript.json."""
    d=pathlib.Path(d); score_dir(d); byid={c["id"]:c for c in cases}
    for sub in [d]+sorted(p for p in d.iterdir() if p.is_dir()):
        summ={}
        for f in sorted(sub.glob("C*.html"))+sorted(sub.glob("C*.transcript.json")):
            cid=f.name.split(".")[0]
            if cid not in byid: continue
            rec={"html":f.read_text()} if f.suffix==".html" else json.loads(f.read_text())
            s=score_case(byid[cid],rec); (sub/f"{cid}.score.json").write_text(json.dumps(s,indent=1)); summ[cid]=brief(s)
        if summ: (sub/"summary.json").write_text(json.dumps(summ,indent=1)); print(sub.name,json.dumps(summ,indent=1))
def brief(s): return {k:s.get(k) for k in("route_gold","route_pred","route_ok","leak","cite_ok","fact_ok","tools_used","wall_s")}

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--models",default="gemma4-26b-a4b:ud-q4_k_xl-cg,qwen3.8:27b-mlx-cg,qwen3.6:35b-a3b-cg")
    ap.add_argument("--out",default=str(H/"out")); ap.add_argument("--dry-run",action="store_true")
    ap.add_argument("--quick",action="store_true",help="first model only, C0 only unless --cases, max_tokens 4000")
    ap.add_argument("--cases",help="comma list, e.g. C0,C1"); ap.add_argument("--max-tokens",type=int,default=8000)
    ap.add_argument("--score",metavar="DIR"); a=ap.parse_args()
    cases=json.loads((H/"cases.json").read_text())
    if a.score: rescore(a.score,cases); return
    want=a.cases.split(",") if a.cases else (["C0"] if a.quick else None)
    if want: cases=[c for c in cases if c["id"] in want]
    models=a.models.split(","); mt=4000 if a.quick else a.max_tokens
    if a.quick: models=models[:1]
    out=pathlib.Path(a.out)
    if a.dry_run:
        print(f"plan: {len(cases)} cases x {len(models)} models, max_tokens {mt}, max tool turns {MAX_TOOL_TURNS}, out {out}")
        for lab,m in zip("ABCDEFGH",models): print(f"  {lab} = {m}")
        print(f"private docs: {[d['doc_id'] for d in list_private()]}  web docs: {[did(p) for p in sorted(WEB.glob('*.txt'))]}")
        for c in cases:
            q=f"{len(PROMPT.format(docs=docs()))} char stuffed prompt -> HTML" if c["mode"]=="stuffed_html" else c["question"]
            print(f"{c['id']} [{c['mode']}] gold={c['route_gold']}\n  q: {q}\n  allow={c['allow_tools']} forbid={c['forbid_tools']}"
                  f"\n  expect private={c['expect_private_cites']} web={c['expect_web_cites']} must_say_absent={c['must_say_absent']}")
        return
    for lab,m in zip("ABCDEFGH",models):
        od=out/lab; od.mkdir(parents=True,exist_ok=True); summ={}
        for c in cases:
            print(f"starting {lab}={m} {c['id']} ...",flush=True)
            if c["mode"]=="stuffed_html":
                rec=run_c0(m,mt); (od/f"{c['id']}.raw.json").write_text(json.dumps(rec.pop("raw"),indent=1))
                (od/f"{c['id']}.html").write_text(rec["html"])
            else:
                rec=run_tools(c,m,mt); (od/f"{c['id']}.transcript.json").write_text(json.dumps(rec,indent=1))
            s=score_case(c,rec); s.update(model=m,err=rec.get("err"))
            (od/f"{c['id']}.score.json").write_text(json.dumps(s,indent=1)); summ[c["id"]]=brief(s)
            print(json.dumps(summ[c["id"]]),flush=True)
        ps=subprocess.run(["ollama","ps"],capture_output=True,text=True).stdout
        (od/"summary.json").write_text(json.dumps({"label":lab,"model":m,"cases":summ,
            "route_ok":sum(1 for v in summ.values() if v["route_ok"]),"n":len(summ),"ollama_ps":ps},indent=1))
        subprocess.run(["ollama","stop",m])
if __name__=="__main__": main()
