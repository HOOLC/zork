/* Pixel-sized resizable sidebar and content-sized composer. */
'use strict';
const sidebarMin=200,sidebarMax=420;
let sidebarWidth=240;
try{const saved=Number(localStorage.getItem('zork-sidebar-width'));if(Number.isFinite(saved)&&saved>=sidebarMin)sidebarWidth=saved;}catch{}
function setSidebarWidth(value,save=false){
 sidebarWidth=Math.max(sidebarMin,Math.min(sidebarMax,value));
 document.documentElement.style.setProperty('--sidebar-width',sidebarWidth+'px');
 const handle=$('sidebar-resizer');handle.setAttribute('aria-valuenow',String(Math.round(sidebarWidth)));
 if(save)try{localStorage.setItem('zork-sidebar-width',String(sidebarWidth));}catch{}
 resizeComposer();
}
function resizeComposer(){
 const field=document.querySelector('#message-form textarea');if(!field)return;
 field.style.height='auto';const limit=160;const height=Math.max(32,Math.min(limit,field.scrollHeight));field.style.height=height+'px';field.style.overflowY=field.scrollHeight>limit?'auto':'hidden';
}
let resizingSidebar=false;
$('sidebar-resizer').addEventListener('pointerdown',e=>{if(e.button!==0)return;e.preventDefault();resizingSidebar=true;document.body.classList.add('resizing-sidebar');e.currentTarget.setPointerCapture(e.pointerId);});
document.addEventListener('pointermove',e=>{if(resizingSidebar)setSidebarWidth(e.clientX);});
document.addEventListener('pointerup',()=>{if(!resizingSidebar)return;resizingSidebar=false;document.body.classList.remove('resizing-sidebar');setSidebarWidth(sidebarWidth,true);});
$('sidebar-resizer').addEventListener('pointercancel',()=>{resizingSidebar=false;document.body.classList.remove('resizing-sidebar');});
$('sidebar-resizer').addEventListener('keydown',e=>{if(e.key==='ArrowLeft'||e.key==='ArrowRight'){e.preventDefault();setSidebarWidth(sidebarWidth+(e.key==='ArrowLeft'?-10:10),true);}});
document.addEventListener('input',e=>{if(e.target.matches('#message-form textarea'))resizeComposer();});
window.addEventListener('resize',resizeComposer);
if(typeof ResizeObserver!=='undefined'){const observer=new ResizeObserver(resizeComposer);observer.observe($('composer-dock'));}
setSidebarWidth(sidebarWidth);
