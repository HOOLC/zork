import { mkdir, symlink, lstat, readFile, writeFile, unlink } from 'node:fs/promises';
import { dirname, resolve, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
const root=resolve(dirname(fileURLToPath(import.meta.url)),'..');
const publicRoot=resolve(root,'public');
const repo=resolve(root,'../..');
await mkdir(publicRoot,{recursive:true});
async function link(source,target){
 await mkdir(dirname(target),{recursive:true});
 try { const info=await lstat(target); if(info.isSymbolicLink())await unlink(target); else throw new Error('Generated link target is not a symlink: '+target); } catch(error) { if(error.code!=='ENOENT')throw error; }
 await symlink(relative(dirname(target),source),target);
}
for(const name of ['assets','components','docs','mobile','motion','wordmark','tokens','licenses','previews','implementation'])await link(resolve(root,name),resolve(publicRoot,name));
const comparisonFont=resolve(repo,'crates/zork-gui-web/assets/NotoSansSC.ttf');
try { await lstat(comparisonFont); await link(comparisonFont,resolve(publicRoot,'comparison-fonts/NotoSansSC.ttf')); } catch(error) { if(error.code!=='ENOENT')throw error; console.warn('Optional GPUI comparison font is not present; system font fallback will be used.'); }
await link(resolve(root,'archive/reference-prototype'),resolve(publicRoot,'reference-prototype'));
await link(resolve(root,'archive/design-coverage-before-2026-09-07.md'),resolve(publicRoot,'archive/design-coverage-before-2026-09-07.md'));
console.log('Prepared design assets, GPUI build and original HTML reference.');

const catalog=JSON.parse(await readFile(resolve(root,'components/manifest.json'),'utf8'));
catalog.fixture=JSON.parse(await readFile(resolve(repo,'crates/zork-ui/assets/stories/page-fixture.json'),'utf8'));
catalog.providers=JSON.parse(await readFile(resolve(repo,'crates/zork-gui/tests/fixtures/provider_catalog.json'),'utf8')).providers;
// Read the renderer's constants so HTML reference colors and control geometry do not drift.
const nativeDesign=await readFile(resolve(repo,'crates/zork-ui/src/design.rs'),'utf8');
const palette=nativeDesign.slice(nativeDesign.indexOf('palette: Palette {',nativeDesign.indexOf('pub const CUE_UI:')));
const colors=Object.fromEntries([...palette.slice(0,palette.indexOf('},')).matchAll(/(\w+): 0x([0-9A-Fa-f]{6})/g)].map(([,name,value])=>[name,'#'+value]));
const nativeControls=await readFile(resolve(repo,'crates/zork-ui/src/controls.rs'),'utf8');
const liquidTokens=await readFile(resolve(repo,'crates/zork-liquid/src/tokens.rs'),'utf8');
const sizeExpressions=Object.fromEntries([...liquidTokens.matchAll(/pub const (\w+): f32 = ([\w.]+);/g),...nativeControls.matchAll(/pub const (\w+): f32 = ([\w.]+);/g)].map(([,name,value])=>[name,value]));
function sizeValue(name,seen=new Set()) { if(seen.has(name))throw new Error('Cyclic control size: '+name); seen.add(name); const value=sizeExpressions[name]; if(value===undefined)throw new Error('Unknown control size: '+name); return /^[0-9.]+$/.test(value)?Number(value):sizeValue(value,seen); }
const sizes=Object.fromEntries(Object.keys(sizeExpressions).map(name=>[name,sizeValue(name)]));
catalog.tokens={colors,sizes};
await writeFile(resolve(publicRoot,'ui-tokens.json'),JSON.stringify(catalog.tokens,null,2)+'\n');
catalog.buildId=String(Date.now());
await writeFile(resolve(publicRoot,'catalog.json'),JSON.stringify(catalog));
