#!/usr/bin/env python3
"""Build the shared Rust component examples for GPUI Web, with a pinned binding tool."""
import argparse,gzip,json,os,shutil,subprocess,tarfile,urllib.request
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]

import sys
sys.path.insert(0, str(ROOT / 'scripts/lib'))
from build_env import build_environment
BINDGEN='0.2.127'

def main():
 p=argparse.ArgumentParser();p.add_argument('--output',type=Path,default=ROOT/'apps/zork-design/components/web');p.add_argument('--release',action='store_true',help='Optimize the complete Web runtime and Rust standard library for performance acceptance');args=p.parse_args();output=args.output;output.mkdir(parents=True,exist_ok=True)
 env=dict(build_environment(),CARGO_INCREMENTAL='0',CARGO_PROFILE_DEV_DEBUG='0',CARGO_BUILD_JOBS='4')
 target_root=Path(env.get('CARGO_TARGET_DIR',ROOT/'target')).resolve()
 sysroot=Path(subprocess.check_output(['rustc','--print','sysroot'],text=True).strip())
 build=['cargo','build','--locked','--target','wasm32-unknown-unknown','-p','zork-gui-web']
 if args.release:
  build.append('--release')
  env['CARGO_PROFILE_RELEASE_LTO']='thin'
  env['CARGO_PROFILE_RELEASE_CODEGEN_UNITS']='1'
 if not (sysroot/'lib/rustlib/wasm32-unknown-unknown').exists():
  if not (sysroot/'lib/rustlib/src/rust/library/Cargo.lock').exists():raise SystemExit('Install the wasm32 Rust target or rust-src before building GPUI Web.')
  env['RUSTC_BOOTSTRAP']='1';build[2:2]=['-Z','build-std=std,panic_abort']
 if 'CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER' not in env and Path('/opt/homebrew/Cellar/lld/23.1.0/bin/wasm-ld').exists():env['CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_LINKER']=str(ROOT/'scripts/storybook/wasm-linker.sh')
 subprocess.run(['uv','run',str(ROOT/'scripts/storybook/prepare_fonts.py')],cwd=ROOT,check=True)
 subprocess.run(build,cwd=ROOT,env=env,check=True)
 cache=target_root/'storybook-tools';tool=cache/f'wasm-bindgen-{BINDGEN}-aarch64-apple-darwin/wasm-bindgen'
 if not tool.exists():
  prior=next((p for p in [ROOT/'target/storybook-tools'/tool.parent.name,Path('/tmp/zork-wasm-tools')/tool.parent.name] if p.is_dir()),Path('/tmp/zork-wasm-tools')/tool.parent.name)
  cache.mkdir(parents=True,exist_ok=True)
  if prior.is_dir():shutil.copytree(prior,tool.parent,dirs_exist_ok=True)
  else:
   archive=cache/'wasm-bindgen.tar.gz';urllib.request.urlretrieve(f'https://github.com/wasm-bindgen/wasm-bindgen/releases/download/{BINDGEN}/wasm-bindgen-{BINDGEN}-aarch64-apple-darwin.tar.gz',archive)
   with tarfile.open(archive) as tar:tar.extractall(cache,filter='data')
 version=subprocess.check_output([str(tool),'--version'],text=True)
 if BINDGEN not in version:raise SystemExit('wasm-bindgen does not match Cargo.lock')
 profile='release' if args.release else 'debug'
 subprocess.run([str(tool),'--target','web','--no-typescript','--out-dir',str(output/'pkg'),str(target_root/'wasm32-unknown-unknown'/profile/'zork_gui_web.wasm')],check=True)
 wasm=output/'pkg/zork_gui_web_bg.wasm';wasm.with_suffix('.wasm.gz').write_bytes(gzip.compress(wasm.read_bytes(),compresslevel=6,mtime=0))
 for name in ['index.html','main.js','client_trace.js','client_probe.js','path_store_probe.js']:shutil.copy2(Path(__file__).parent/'web'/name,output/name)
 brand=output/'assets/brand';brand.mkdir(parents=True,exist_ok=True);shutil.copy2(ROOT/'crates/zork-ui/assets/brand/mark.svg',brand/'mark.svg')
 shutil.copy2(Path(__file__).parent/'webgpu_timing.js',output/'webgpu_timing.js')
 (output/'build.json').write_text(json.dumps({'package':'zork-gui-web','component_package':'zork-ui','profile':'release' if args.release else 'dev','lto':'thin' if args.release else False,'codegen_units':1 if args.release else None,'gpui':'1.17.0-pre','wasm_bindgen':BINDGEN,'wasm_bytes':wasm.stat().st_size,'compressed_bytes':wasm.with_suffix('.wasm.gz').stat().st_size,'network':'in-memory fixtures only','cjk_font':'Noto Sans SC (OFL); native system fallback remains PingFang SC'},indent=2)+'\n')
 print('GPUI Web ready:',output)
if __name__=='__main__':main()
