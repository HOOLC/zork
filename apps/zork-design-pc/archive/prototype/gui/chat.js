/* Local HTML concept: Agents organize the work; people converse and inspect. */
'use strict';
const $=id=>document.getElementById(id);
const esc=value=>String(value??'').replace(/[&<>"']/g,c=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const common=new Set(['search','plus','chevron-down','arrow-left','arrow-up','x','settings','download','inbox','bars-three','panel-left','paperclip','check','task-chat','mention']);
const icon=(n)=>`<span class="icon" style="--icon:url(../assets/icons/${common.has(n)?'interface/':'product/'}${n}.svg)" aria-hidden="true"></span>`;
const av=(name,cls='')=>`<img class="${cls}" src="../assets/avatars/${name}.svg" alt="">`;
const avatarIds=['cat','bunny','bear','fox','panda','chick','dog','owl','koala','penguin','deer','octopus'];
const iconIds=['node','leader','worker','mesh','task','handoff','result','review','history','workspace','permission','offline'];
const people={
 product:{name:'产品 Leader',avatar:'fox',role:'Leader',node:'mini1'},engineering:{name:'工程 Leader',avatar:'dog',role:'Leader',node:'mini1'},research:{name:'研究 Leader',avatar:'owl',role:'Leader',node:'mini2'},
 design:{name:'设计 Worker',avatar:'cat',role:'Worker',node:'mini2'},illustration:{name:'插画 Worker',avatar:'panda',role:'Worker',node:'mini2'},client:{name:'客户端 Worker',avatar:'koala',role:'Worker',node:'mini1'},docs:{name:'文档 Worker',avatar:'deer',role:'Worker',node:'mini2'},test:{name:'测试 Worker',avatar:'penguin',role:'Worker',node:'mini1'},data:{name:'数据 Worker',avatar:'octopus',role:'Worker',node:'mini2'},assistant:{name:'协作 Worker',avatar:'bunny',role:'Worker',node:'mini1'},build:{name:'构建 Worker',avatar:'bear',role:'Worker',node:'mini1'},ideas:{name:'创意 Worker',avatar:'chick',role:'Worker',node:'mini2'}
};
const devices={mini1:{name:'mini1',description:'开发设备',online:true},mini2:{name:'mini2',description:'辅助设备',online:true}};
const collapsedDevices=new Set();
const collapsedLeaders=new Set();
const files={icons:{name:'zork-interface-icons.zip',meta:'12 个功能图标 · SVG',href:'downloads/zork-interface-icons.zip'},avatars:{name:'zork-avatars-12.zip',meta:'12 个头像 · SVG + PNG',href:'downloads/zork-avatars-12.zip'},guide:{name:'brand-guidelines.md',meta:'品牌与资源规范',href:'proposal.md'}};
let sequence=100;
const m=(who,text,extra={})=>({id:'m'+(++sequence),who,text,time:'09:42',...extra});
const initial=[
 {id:'product',name:'产品 Leader',members:['product'],unread:0,time:'昨天',messages:[m('product','想法、问题，或者还没想清楚的事，都可以直接在这里聊。'),m('me','后面我们把品牌和界面一起打磨。'),m('product','好。我会按需要创建 Task、邀请合适的伙伴，并安排执行设备。你直接在对话里说想法就行。')]},
 {id:'engineering',name:'工程 Leader',members:['engineering'],unread:0,time:'昨天',messages:[m('engineering','有实现上的想法，随时找我。'),m('me','离线恢复的策略我们单开一个群聊吧。'),m('engineering','我已创建「离线恢复怎么处理」Task，邀请客户端 Worker 一起处理。进展会留在那个群里。')]},
 {id:'brand',name:'品牌资源接入',members:['product','design','illustration'],unread:0,time:'刚刚',messages:[m('me','这轮先把图标和头像做统一，保持苹果原生的感觉。',{time:'09:40'}),m('product','收到。我把设计和插画伙伴拉进来了，大家直接在这里对齐。',{time:'09:41'}),m('system','产品 Leader 创建了此 Task · mini1 · 已邀请设计 Worker、插画 Worker'),m('design','功能图标已经按新的比例和圆角整理了一版。',{time:'09:43',files:['icons']}),m('illustration','我也补齐了 12 个头像。先看整体感觉，小尺寸的版本也放在压缩包里了。',{time:'09:45',files:['avatars']}),m('me','可以，后面的 GUI 样稿就用这套资源。',{time:'09:47'}),m('design','好，图标和头像都会直接复用。布局我们从实际怎么协作来想，不套固定流程。',{time:'09:48'})]},
 {id:'offline',name:'离线恢复怎么处理',members:['engineering','client'],unread:2,time:'10 分钟前',messages:[m('me','断线以后，已经看到的内容不要突然消失。'),m('client','同意。已同步的消息和文件继续保留，连接状态单独显示。'),m('engineering','有副作用的操作先别自动重跑，我们在这个群里把边界聊清楚。')]},
 {id:'guides',name:'品牌规范整理',members:['product','docs'],unread:1,time:'稍早',messages:[m('docs','把这轮确定下来的资源使用方式整理成了一份文档。',{files:['guide']}),m('product','后面新增图标和头像，都可以在这里继续补充。')]},
 {id:'research',name:'研究 Leader',members:['research'],unread:0,time:'昨天',messages:[m('research','需要查资料或理清思路，可以直接发给我。')]}
];
initial.forEach(c=>{c.node=people[c.members[0]].node;c.kind=c.members.length===1&&people[c.members[0]].role==='Leader'?'leader':'task';c.leaderId=c.members.find(id=>people[id].role==='Leader');});
let conversations=structuredClone(initial);
let current='brand',panel=null,reply=null;
let drafts={};
const chat=()=>conversations.find(c=>c.id===current);
const nameOf=id=>id==='me'?'你':people[id]?.name||'群内消息';
function groupIcon(c){return c.kind==='leader'?av(people[c.members[0]].avatar,'single-avatar leader-chat-avatar'):'';}
function leaderConversation(id){
 let c=conversations.find(c=>c.kind==='leader'&&c.leaderId===id);
 if(!c){const p=people[id];c={id:'leader-'+id,name:p.name,kind:'leader',leaderId:id,node:p.node,members:[id],unread:0,time:'',messages:[]};conversations.push(c);}
 return c;
}
function renderList(){
 const row=c=>`<button class="conversation-row ${c.kind}-conversation ${c.unread?'is-unread':''} ${c.id===current?'selected':''}" data-action="chat" data-chat="${c.id}" data-kind="${c.kind}" aria-label="${c.kind==='leader'?'Leader 对话':'任务群聊'}：${esc(c.name)}">${groupIcon(c)}<span class="conversation-title"><b>${esc(c.name)}</b></span>${c.unread?`<span class="unread-count">${c.unread}</span>`:''}</button>`;
 $('conversation-list').innerHTML=Object.entries(devices).map(([node,d])=>{
   const scopes=Object.entries(people).filter(([,p])=>p.role==='Leader'&&p.node===node).map(([id])=>{
     const direct=leaderConversation(id);
     const allTasks=conversations.filter(c=>c.kind==='task'&&c.leaderId===id);
     const tasks=allTasks.slice().sort((a,b)=>Number(b.unread>0)-Number(a.unread>0));
     return {id,direct,tasks,unread:direct.unread+allTasks.reduce((n,t)=>n+t.unread,0)};
   }).sort((a,b)=>Number(b.unread>0)-Number(a.unread>0));
   const closed=collapsedDevices.has(node);
   const content=scopes.map(scope=>{
     const folded=collapsedLeaders.has(scope.id);
     return `<div class="leader-scope" data-leader="${scope.id}"><div class="leader-scope-heading ${scope.direct.id===current?'selected':''}">${row(scope.direct)}${scope.tasks.length?`<button class="icon-button scope-toggle ${folded?'collapsed':''} ${folded&&scope.tasks.some(t=>t.unread)?'has-unread':''}" data-action="collapse-leader" data-leader="${scope.id}" aria-label="${folded?'展开':'收起'} ${esc(people[scope.id].name)} 下的 Task" aria-expanded="${!folded}" aria-controls="leader-tasks-${scope.id}">${icon('chevron-down')}</button>`:''}</div>${scope.tasks.length?`<div class="leader-tasks" id="leader-tasks-${scope.id}" ${folded?'hidden':''}>${scope.tasks.map(row).join('')}</div>`:''}</div>`;
   }).join('');
   return `<section class="device-group" aria-label="${d.name} 上的对话"><div class="device-group-header"><button class="device-heading" data-action="collapse-device" data-device="${node}" aria-expanded="${!closed}" aria-controls="device-conversations-${node}">${icon('node')}<strong>${d.name}</strong><span class="device-connection"><i></i>在线</span></button></div><div id="device-conversations-${node}" class="device-conversations" ${closed?'hidden':''}>${content||'<div class="device-empty">暂无对话</div>'}</div></section>`;
 }).join('');
}
function renderHeader(){
 const c=chat();
 document.querySelector('.conversation-main').setAttribute('aria-label',c.name);
 $('conversation-header').innerHTML=`<div class="header-roster"><div class="member-avatars" role="group" aria-label="对话成员">${c.members.map(id=>{const p=people[id];return `<button class="header-member" data-action="panel" data-panel="members" aria-label="${esc(p.name)}，查看成员" title="${esc(p.name)} · ${p.node}">${av(p.avatar)}</button>`;}).join('')}</div></div><button class="icon-button mobile-back" data-action="show-list" aria-label="返回对话列表">${icon('arrow-left')}</button><button class="chat-device header-device" data-action="device" data-device="${c.node}" title="对话所在设备：${c.node}">${icon('node')}${c.node}</button>`;
}
function attachment(id){const f=files[id];return `<button class="attachment" data-action="file" data-file="${id}"><div class="attachment-top">${icon('result')}<div><div class="attachment-name">${f.name}</div><div class="attachment-meta">${f.meta}</div></div></div>${id==='avatars'?`<div class="attachment-sample">${avatarIds.slice(0,6).map(a=>av(a)).join('')}</div>`:id==='icons'?`<div class="attachment-sample">${['node','leader','worker','task','review'].map(icon).join('')}</div>`:''}</button>`;}
function renderMessage(msg){
 if(msg.who==='system')return `<div class="system-line" id="${msg.id}">${esc(msg.text)}</div>`;
 const p=people[msg.who],mine=msg.who==='me';
 const content=`${msg.quote?`<div class="quote">${esc(msg.quote)}</div>`:''}${msg.text?`<p class="message-text">${esc(msg.text)}</p>`:''}${msg.comments?.length?renderMessageComments(msg.comments):''}${msg.files?.length?`<div class="attachments">${msg.files.map(attachment).join('')}</div>`:''}`;

 if(mine)return `<article class="chat-message mine" id="${msg.id}" aria-label="你发送的消息"><div class="message-body">${content}</div><div class="own-message-meta"><time>${msg.time}</time></div></article>`;
 return `<article class="chat-message" id="${msg.id}">${av(p.avatar,'message-avatar')}<div class="message-body"><div class="message-byline"><b>${esc(p.name)}</b><button class="message-device" data-action="device" data-device="${p.node}" title="${esc(p.name)} 运行于 ${p.node}">${p.node}</button><time>${msg.time}</time></div>${content}</div></article>`;
}
function renderMessages(){const c=chat();$('messages').innerHTML=c.messages.length?'<div class="date-divider">今天</div>'+c.messages.map(renderMessage).join(''):`<div class="chat-empty">大家已在群里，可以开始聊了。</div>`;}
function renderComposer(){const c=chat();$('composer-dock').innerHTML=`<div class="participant-activity"></div><div class="pending-comments" id="pending-comments" hidden></div><form class="composer" id="message-form">${reply?`<div class="reply-preview"><span>回复 ${esc(nameOf(reply.who))}：${esc(reply.text)}</span><button class="icon-button" type="button" data-action="cancel-reply" aria-label="取消回复">${icon('x')}</button></div>`:''}<textarea name="message" maxlength="4000" placeholder="${c.kind==='task'?'补充想法，或告诉 Agent 接下来怎么做…':'告诉 Leader 你想做什么…'}" aria-label="消息">${esc(drafts[current]||'')}</textarea><div class="composer-bottom"><div class="composer-tools"><button class="icon-button" type="button" data-action="share-file" aria-label="分享文件" title="分享文件">${icon('paperclip')}</button></div><span class="composer-hint">${typeof preferences!=='undefined'&&preferences.send==='modifier'?'⌘ / Ctrl Enter':'Enter'} 发送 · Shift Enter 换行</span><button class="send-button" type="submit" aria-label="发送消息">${icon('arrow-up')}</button></div></form>`;if(typeof renderPendingComments==='function')renderPendingComments();if(typeof resizeComposer==='function')resizeComposer();}
function renderPanel(){
 const el=$('context-panel');el.hidden=!panel;if(!panel)return;
 const c=chat(),titles={members:'群聊成员'};
 let body='';
 if(panel==='members')body=`<div class="panel-label">${c.members.length+1} 位成员</div><div class="member"><span class="person-placeholder">${icon('worker')}</span><span class="member-info"><b>你</b><small>在这个群里一起协作</small></span></div>${c.members.map(id=>{const p=people[id];return `<div class="member">${av(p.avatar)}<span class="member-info"><b>${esc(p.name)}</b><small><button data-action="device" data-device="${p.node}">${p.node}</button></small></span></div>`;}).join('')}<p class="panel-note">${c.kind==='task'?'Task 属于 '+esc(people[c.leaderId].name)+' 的长期范围。':''}对话位于 ${c.node}。成员由 Leader 根据工作需要邀请，在各自的设备上运行。</p>`;
 el.innerHTML=`<div class="panel-head"><span>${titles[panel]}</span><button class="icon-button" data-action="close-panel" aria-label="关闭侧栏">${icon('x')}</button></div><div class="panel-body">${body}</div>`;
}
function render(){renderList();renderHeader();renderMessages();renderComposer();renderPanel();}
function selectChat(id){if(typeof closeSelectionReply==='function')closeSelectionReply();if(typeof closeSettings==='function')closeSettings();const c=conversations.find(c=>c.id===id);if(!c)return;current=id;c.unread=0;collapsedDevices.delete(c.node);collapsedLeaders.delete(c.leaderId);if($('chat-dialog').open)$('chat-dialog').close();reply=null;panel=null;document.body.classList.remove('mobile-list');render();$('message-scroll').scrollTop=0;}
function modal(title,body){$('dialog-content').innerHTML=`<div class="modal-head"><h2 id="dialog-title">${esc(title)}</h2><button class="icon-button" data-action="close-dialog" aria-label="关闭">${icon('x')}</button></div><div class="modal-body">${body}</div>`;if(!$('chat-dialog').open)$('chat-dialog').showModal();}
function deviceDetails(id){
 const d=devices[id];if(!d)return;
 const owned=Object.keys(people).filter(pid=>people[pid].role==='Leader'&&people[pid].node===id).flatMap(pid=>[leaderConversation(pid),...conversations.filter(c=>c.kind==='task'&&c.leaderId===pid)]),residents=Object.entries(people).filter(([,p])=>p.node===id);
 modal(d.name,`<div class="device-summary">${icon('node')}<div><strong>${d.description}</strong><span><i></i>在线 · 模型连接可用</span></div></div><p>对话有自己的所在设备；群内成员在各自设备上运行，两者可以不同。</p><div class="panel-label">本设备的对话</div>${owned.map(c=>`<button class="device-chat-link ${c.kind==='task'?'task-link':''}" data-action="chat" data-chat="${c.id}">${groupIcon(c)}<span>${esc(c.name)}</span></button>`).join('')||'<p>暂无对话。</p>'}<div class="panel-label device-members-heading">在本设备运行的伙伴</div><div class="device-member-grid">${residents.map(([,p])=>`<div class="member">${av(p.avatar)}<span class="member-info"><b>${esc(p.name)}</b></span></div>`).join('')}</div>`);
}
function previewFile(id){const f=files[id];if(!f)return;const body=id==='avatars'?`<div class="modal-avatar-grid">${avatarIds.map(a=>av(a)).join('')}</div>`:id==='icons'?`<div class="modal-icons">${iconIds.map(i=>`<div>${icon(i)}</div>`).join('')}</div>`:`<div class="guide-text"><h3>品牌、角色和个体</h3><p>zork 表示产品，Leader / Worker 表示角色，头像表示群里的具体伙伴。</p><h3>所有资源保留源文件</h3><p>图标和头像保留 SVG，方便调整与复用；位图用于预览和分享。</p><h3>在对话里继续完善</h3><p>设计决定、讨论和文件都可以留在群里，随时接着改。</p></div>`;modal(f.name,body+`<div class="modal-actions"><a class="button primary" href="${f.href}" download>${icon('download')}下载文件</a></div>`);}
function addMessage(msg){const c=chat();c.messages.push(msg);c.time='刚刚';renderList();renderMessages();renderPanel();requestAnimationFrame(()=>$('message-scroll').scrollTop=$('message-scroll').scrollHeight);}
document.addEventListener('click',e=>{
 const b=e.target.closest('[data-action]');if(!b)return;
 const action=b.dataset.action;
 if(action==='chat')selectChat(b.dataset.chat);
 else if(action==='collapse-device'){const id=b.dataset.device;collapsedDevices.has(id)?collapsedDevices.delete(id):collapsedDevices.add(id);renderList();}
 else if(action==='collapse-leader'){const id=b.dataset.leader;collapsedLeaders.has(id)?collapsedLeaders.delete(id):collapsedLeaders.add(id);renderList();}
 else if(action==='leader-chat')selectChat(leaderConversation(b.dataset.leader).id);
 else if(action==='device')deviceDetails(b.dataset.device);
 else if(action==='show-list')document.body.classList.add('mobile-list');
 else if(action==='panel'){panel=panel===b.dataset.panel?null:b.dataset.panel;renderHeader();renderPanel();}
 else if(action==='close-panel'){panel=null;renderHeader();renderPanel();}
 else if(action==='close-dialog')$('chat-dialog').close();
 else if(action==='file')previewFile(b.dataset.file);
 else if(action==='cancel-reply'){reply=null;renderComposer();}
 else if(action==='share-file')modal('分享文件',`<p>从当前样稿已有的资源中选择一份，发到群里。</p>${Object.entries(files).map(([id,f])=>`<button class="panel-file" data-action="send-file" data-file="${id}">${icon('result')}<span><b>${f.name}</b><small>${f.meta}</small></span></button>`).join('')}`);
 else if(action==='send-file'){$('chat-dialog').close();addMessage(m('me','分享了一份文件',{files:[b.dataset.file],time:'刚刚'}));}
});
document.addEventListener('input',e=>{if(e.target.matches('#message-form textarea'))drafts[current]=e.target.value;});
document.addEventListener('submit',e=>{
 e.preventDefault();const form=e.target,data=new FormData(form);
 if(form.id==='message-form'){const text=String(data.get('message')||'').trim(),comments=typeof getPendingComments==='function'?getPendingComments(current):[];if(!text&&!comments.length)return;drafts[current]='';reply=null;addMessage(m('me',text,{comments,time:'刚刚'}));if(typeof clearPendingComments==='function')clearPendingComments(current);renderComposer();document.querySelector('#message-form textarea').focus();}

});
document.addEventListener('keydown',e=>{if(e.key==='Enter'&&!e.shiftKey&&!e.isComposing&&e.target.matches('#message-form textarea')&&(typeof preferences==='undefined'||preferences.send==='enter'||e.metaKey||e.ctrlKey)){e.preventDefault();$('message-form').requestSubmit();}if(e.key==='Escape'&&panel){panel=null;renderHeader();renderPanel();}});
$('chat-dialog').addEventListener('click',e=>{if(e.target===$('chat-dialog')){const r=$('chat-dialog').getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)$('chat-dialog').close();}});
render();
