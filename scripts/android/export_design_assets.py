"""Export the approved SVG paths to Android vector drawables, without redrawing."""
from pathlib import Path
import re
import xml.etree.ElementTree as ET
ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / 'apps/android/app/src/main/res/drawable'
NS = 'http://schemas.android.com/apk/res/android'
ET.register_namespace('android', NS)
def attr(node, name, value): node.set('{'+NS+'}'+name, str(value))
def export(source, name):
    svg=ET.parse(source).getroot(); vb=[float(v) for v in svg.attrib['viewBox'].split()]
    vector=ET.Element('vector')
    for k,v in [('width',f'{vb[2]:g}dp'),('height',f'{vb[3]:g}dp'),('viewportWidth',f'{vb[2]:g}'),('viewportHeight',f'{vb[3]:g}')]: attr(vector,k,v)
    def visit(el,parent,style):
        tag=el.tag.split('}')[-1]; st={**style,**el.attrib}
        if tag in ['title','desc']: return
        target=parent
        if el.attrib.get('transform'):
            match=re.fullmatch(r'rotate\(([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\)',el.attrib['transform'])
            assert match, el.attrib['transform']
            target=ET.SubElement(parent,'group')
            for k,v in zip(['rotation','pivotX','pivotY'],match.groups()): attr(target,k,v)
        if tag in ['g','svg']:
            for child in el: visit(child,target,st)
            return
        get=lambda key,default=0:float(el.attrib.get(key,default))
        if tag=='path': d=el.attrib['d']
        elif tag in ['ellipse','circle']:
            x,y=get('cx'),get('cy');rx=get('rx',get('r'));ry=get('ry',get('r'))
            d=f'M{x-rx} {y}a{rx} {ry} 0 1 0 {rx*2} 0a{rx} {ry} 0 1 0 {-rx*2} 0Z'
        elif tag=='rect':
            x,y,w,h=get('x'),get('y'),get('width'),get('height');r=min(get('rx'),w/2,h/2)
            d=f'M{x+r} {y}H{x+w-r}A{r} {r} 0 0 1 {x+w} {y+r}V{y+h-r}A{r} {r} 0 0 1 {x+w-r} {y+h}H{x+r}A{r} {r} 0 0 1 {x} {y+h-r}V{y+r}A{r} {r} 0 0 1 {x+r} {y}Z'
        elif tag=='line': d=f'M{get("x1")} {get("y1")}L{get("x2")} {get("y2")}'
        elif tag in ['polyline','polygon']: d='M'+el.attrib['points']+('Z' if tag=='polygon' else '')
        else: raise ValueError(f'Unsupported source element: {tag}: {source}')
        path=ET.SubElement(target,'path');attr(path,'pathData',d)
        color=lambda v:'#00000000' if v=='none' else '#24272B' if v=='currentColor' else v
        attr(path,'fillColor',color(st.get('fill','#24272B')))
        if st.get('fill-rule')=='evenodd': attr(path,'fillType','evenOdd')
        if 'stroke' in st:
            attr(path,'strokeColor',color(st['stroke']));attr(path,'strokeWidth',st.get('stroke-width','1'))
            for a,b in [('stroke-linecap','strokeLineCap'),('stroke-linejoin','strokeLineJoin')]:
                if a in st: attr(path,b,st[a])
        if 'opacity' in st: attr(path,'fillAlpha',st['opacity'])
    visit(svg,vector,{})
    ET.indent(vector)
    (OUT/(name+'.xml')).write_text('<!-- Generated from '+str(source.relative_to(ROOT))+'; do not redraw. -->\n'+ET.tostring(vector,encoding='unicode')+'\n')
def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--icons", nargs="+", help="export only these shared functional icons")
    args = parser.parse_args()
    if args.icons:
        for name in args.icons:
            if not re.fullmatch(r"[a-z0-9-]+", name):
                parser.error("icon names must use lowercase letters, digits and hyphens")
            export(ROOT / f"crates/zork-ui/assets/icons/{name}.svg", "ic_" + name.replace("-", "_"))
        return
    MOBILE = ROOT / 'apps/zork-design-pc/archive/mobile-prototype/assets'
    for source in (MOBILE/'avatars').glob('*.svg'): export(source,'avatar_'+source.stem)
    for name in ['node','arrow-left','arrow-up','plus','settings','paperclip','chevron-down','mesh','x','result','download']:
        export(MOBILE/f'icons/{name}.svg','ic_'+name.replace('-','_'))
    export(ROOT/'crates/zork-ui/assets/icons/phosphor-stop-fill.svg','ic_phosphor_stop_fill')
    export(MOBILE/'brand/zork-wordmark.svg','zork_wordmark')
    export(MOBILE/'mark.svg','ic_zork')

    # Shared current settings assets.
    for name in ["edit", "reload", "attention", "arrow-right", "archive", "archive-restore"]:
        export(ROOT/f"crates/zork-ui/assets/icons/{name}.svg", "ic_"+name.replace("-","_"))
    for source in (ROOT/"crates/zork-ui/assets/providers").glob("*.svg"):
        export(source,"provider_"+source.stem)

    # Composer portraits share the desktop silhouettes without the original avatar disc.
    for source in (ROOT/"crates/zork-ui/assets/avatars/portraits").glob("*.svg"):
        export(source,"portrait_"+source.stem)

    # Execution history uses the same semantic paths as the native history reader.
    for source in (ROOT/"crates/zork-ui/assets/history").glob("*.svg"):
        export(source, "history_"+source.stem.replace("-", "_"))
    export(ROOT/"crates/zork-ui/assets/icons/copy.svg", "ic_copy")

if __name__ == "__main__":
    main()
