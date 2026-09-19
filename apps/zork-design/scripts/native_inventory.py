"""Inventory actual native SVG registration and call sites, preserving provenance."""
from pathlib import Path
import collections, hashlib, html, json, re, shutil
ROOT=Path(__file__).resolve().parents[1]
REPO=ROOT.parents[1]
SOURCES={'current':REPO/'crates/zork-gui'}

def build_native_inventory(shell):
    items=[];summaries={};missing=[]
    for label,source in SOURCES.items():
        shared=source.parent/'zork-ui'
        registry=((shared if shared.exists() else source)/'src/assets.rs').read_text()
        registered=set(re.findall(r'"((?:interface|icons|brand|providers|avatars|illustrations|motion)/[^"\s]+\.svg)"',registry))
        refs={}
        code_roots=[source/'src']+([shared/'src'] if shared.exists() else [])
        for file in (file for root in code_roots for file in root.rglob('*.rs')):
            if file.name=='assets.rs':continue
            for line,text in enumerate(file.read_text().splitlines(),1):
                for path in re.findall(r'"((?:interface|icons|brand|providers|avatars|illustrations|motion)/[^"\s]+\.svg)"',text):
                    refs.setdefault(path,[]).append({'file':str(file.relative_to(source.parent)) if shared.exists() else str(file.relative_to(source)),'line':line})
        files=sorted((source/'assets').rglob('*.svg'))
        summaries[label]={'files':len(files),'registered':len(registered),'direct_reference_paths':len(refs),'families':dict(collections.Counter(p.relative_to(source/'assets').parts[0] for p in files))}
        for path in refs:
            if not (source/'assets'/path).exists():missing.append({'checkout':label,'path':path,'references':refs[path]})
        for p in files:
            relative=p.relative_to(source/'assets').as_posix();target=ROOT/'assets/native'/label/relative;target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(p,target)
            items.append({'checkout':label,'path':relative,'snapshot':f'{label}/{relative}','family':relative.split('/')[0],'registered':relative in registered,'references':refs.get(relative,[]),'sha256':hashlib.sha256(p.read_bytes()).hexdigest(),'source':str(p.relative_to(REPO))})
        for p in [source/'assets/interface/README.md',source/'assets/interface/icons.json',source/'assets/icons/PHOSPHOR_LICENSE.txt',source/'assets/providers/sources.json']:
            if p.exists():
                target=ROOT/'assets/native'/label/p.relative_to(source/'assets');target.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(p,target)
    data={'captured':'2026-09-06','scope':'native SVG files, embedded registrations and literal Rust references; dynamic references may not be detected','summaries':summaries,'missing':missing,'items':items}
    (ROOT/'assets/native/inventory.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
    cards=[]
    for item in items:
        status='当前代码直接引用' if item['references'] else ('来源保留 · 无直接引用证据' if item['family']=='interface' else ('已注册 · 无直接引用证据' if item['registered'] else '目录资源 · 未注册'))
        refs='<br>'.join(f"{html.escape(x['file'])}:{x['line']}" for x in item['references']) or '可能由动态路径使用，或尚未接入。'
        cards.append(f'<article class="asset native-asset" data-checkout="{item["checkout"]}" data-family="{item["family"]}" data-referenced="{bool(item["references"]).__str__().lower()}" data-name="{item["path"]}"><a class="asset-image" href="{item["snapshot"]}"><img src="{item["snapshot"]}" alt="{item["path"]}" loading="lazy"></a><b>{item["path"]}</b><small>{status}</small><details><summary>引用位置</summary><p class="source-list">{refs}</p></details></article>')
    filters=f'''<h1>原生 SVG 完整清单</h1><p>默认显示当前代码直接引用的资源；取消筛选可查看注册候选与历史来源。通用界面资源不代表所有文件都在当前界面使用。</p><div class="inventory-controls"><label>代码版本 <select id="checkout"><option value="current" selected>主代码目录 · {summaries['current']['files']} SVG</option></select></label><label>类型 <select id="family"><option value="">全部</option><option value="interface">通用界面</option><option value="icons">功能图标</option><option value="avatars">头像</option><option value="brand">品牌</option><option value="providers">供应商</option><option value="illustrations">场景插画</option><option value="motion">动效资源</option></select></label><label><input id="referenced" type="checkbox" checked> 只看当前直接引用</label><input id="native-search" class="search" type="search" placeholder="搜索路径" aria-label="搜索原生 SVG"><span id="native-count"></span></div><p><a href="inventory.json">完整 JSON 与源码位置</a> · <a href="../index.html">设计素材总览</a></p><div class="asset-grid">'''
    script='''<script>const controls=['checkout','family','referenced','native-search'].map(id=>document.getElementById(id));function filter(){const [checkout,family,referenced,search]=controls;let count=0;document.querySelectorAll('.native-asset').forEach(item=>{const show=item.dataset.checkout===checkout.value&&(!family.value||item.dataset.family===family.value)&&(!referenced.checked||item.dataset.referenced==='true')&&item.dataset.name.includes(search.value.toLowerCase());item.hidden=!show;if(show)count++;});document.getElementById('native-count').textContent=count+' 个 SVG';}controls.forEach(c=>c.addEventListener('input',filter));filter();</script>'''
    body=filters+''.join(cards)+'</div><p class="muted">Central Icons 图标为项目已授权复用的公司资源，来源见快照中的 README / icons.json；其许可不由 Inter 字体许可替代。Phosphor 保留原 MIT 许可。两个快照中的同名文件可能来自不同设计阶段。</p>'+script
    (ROOT/'assets/native/index.html').write_text(shell('原生 SVG 清单',body,'../../'))
    return data
