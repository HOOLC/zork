#!/usr/bin/env python3
"""Validate/enrich the native manifest; the component browser is owned by React."""
import argparse,hashlib,json
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
def main():
 p=argparse.ArgumentParser();p.add_argument('output',type=Path);args=p.parse_args();out=args.output
 data=json.loads((out/'manifest.json').read_text())
 # Benchmark-only fixtures do not participate in the rendered gallery. Keeping
 # them out also avoids treating measurement-harness fixes as production changes.
 data['production_source_hashes']={str(path.relative_to(ROOT)):hashlib.sha256(path.read_bytes()).hexdigest() for directory in ['crates/zork-gui/src','crates/zork-ui/src'] for path in (ROOT/directory).rglob('*.rs') if path.name not in ['stories.rs','storybook.rs','benchmark.rs','render-bench.rs'] and not any(part in ['benchmark','render-bench'] for part in path.relative_to(ROOT).parts)}
 data.pop('native_compositor_sources', None)
 data['fixture']=json.loads((ROOT/'crates/zork-ui/assets/stories/page-fixture.json').read_text())
 for story in data['stories']:
  if story['family'] not in ['connection','model','agent','conversation','client','device','mesh','enrollment']:story['design']={'status':'not_applicable'}
  else:story.setdefault('design',{'status':'missing','reason':'此状态尚未生成原始 HTML 参考。'})
  for key in ['image','window','geometry']:assert (out/story['native'][key]).is_file(),story['id']
  if story['design']['status']=='captured':assert (out/story['design']['image']).is_file(),story['id']
 (out/'manifest.json').write_text(json.dumps(data,ensure_ascii=False,indent=2)+'\n')
 # Preserve old shared links without maintaining a second application.
 (out/'index.html').write_text('<!doctype html><meta charset="utf-8"><title>Zork Design</title><script>location.replace("../index.html#/pc/"+(location.hash.slice(1)||"button"))</script><a href="../index.html#/pc/button">打开 PC 组件库</a>')
 print(f"Validated {len(data['stories'])} GPUI states for the React design workbench")
if __name__=='__main__':main()
