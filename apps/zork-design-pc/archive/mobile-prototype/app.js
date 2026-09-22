'use strict';
const $ = s => document.querySelector(s);
const escapeHTML = v => String(v ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
const icon = (name, cls='') => `<img class="icon ${cls}" src="assets/icons/${name}.svg" alt="" draggable="false">`;
const avatar = (name, cls='') => `<img class="avatar ${cls}" src="assets/avatars/${name}.svg" alt="" draggable="false">`;
const arrow = () => icon('chevron-down','chevron right');
const state = {
  route:'nav', chat:'brand', device:'mini1', lastChat:'brand', collapsed:new Set(), scroll:{}, selectedText:'', comments:{}, drafts:{}, attachments:{}, messages:{},
  devices:[{id:'mini1',online:true,background:true,startup:true},{id:'mini2',online:true,background:true,startup:false}],
  leaders:[{id:'product',name:'产品 Leader',avatar:'fox',device:'mini1'},{id:'engineering',name:'工程 Leader',avatar:'dog',device:'mini1'},{id:'research',name:'研究 Leader',avatar:'owl',device:'mini2'}],
  tasks:[{id:'guide',name:'品牌规范整理',leader:'product',unread:1},{id:'brand',name:'品牌资源接入',leader:'product',unread:0},{id:'offline',name:'离线恢复怎么处理',leader:'engineering',unread:2},{id:'mobile',name:'移动端交互调研',leader:'research',unread:0}],
  connections:{mini1:[{name:'OpenAI 订阅',provider:'OpenAI',kind:'订阅账号',models:[]},{name:'Anthropic API',provider:'Anthropic',kind:'API 接入',models:[]}],mini2:[]},
  agents:{mini1:[{name:'产品 Leader',avatar:'fox',kind:'Leader'},{name:'工程 Leader',avatar:'dog',kind:'Leader'},{name:'设计 Worker',avatar:'cat',kind:'Worker'},{name:'插画 Worker',avatar:'panda',kind:'Worker'},{name:'客户端 Worker',avatar:'bunny',kind:'Worker'},{name:'验证 Worker',avatar:'bear',kind:'Worker'}],mini2:[{name:'研究 Leader',avatar:'owl',kind:'Leader'}]},
  connectKind:'订阅账号', connectionIndex:0, returnRoute:'settings'
};
const baseMessages = [
 {own:true,text:'移动端也沿用这套品牌，\n阅读和回复要轻一点。',time:'10:24'},
 {name:'产品 Leader',avatar:'fox',device:'mini1',text:'收到。导航和群聊分开，\n手机上一次专注一件事。',time:'10:25'},
 {name:'设计 Worker',avatar:'cat',device:'mini2',text:'三屏稿整理好了，可以先看整体。',time:'10:27',file:'zork-mobile-notes.md'},
 {name:'插画 Worker',avatar:'panda',device:'mini2',text:'头像直接复用，保留每位伙伴的辨识度。',time:'10:28'}
];
state.messages.brand = baseMessages;
state.comments.brand = [{quote:'导航和群聊分开',text:'切换后保留阅读位置。'}];
const currentDevice = () => state.devices.find(d => d.id===state.device) || state.devices[0];
const chatLeader = id => state.leaders.find(l => l.id===id) || state.leaders.find(l => l.id===state.tasks.find(t=>t.id===id)?.leader) || state.leaders[0];
const currentComments = () => state.comments[state.chat] ||= [];
const currentAttachments = () => state.attachments[state.chat] ||= [];
function timeNow(){return new Date().toLocaleTimeString('zh-CN',{hour:'2-digit',minute:'2-digit',hour12:false});}
function toast(message){$('#toast').textContent=message;$('#toast').classList.add('visible');clearTimeout(toast.timer);toast.timer=setTimeout(()=>$('#toast').classList.remove('visible'),3000);}
function navigate(route,extra={}){
 state.scroll[location.hash || '#nav']=$('#main').scrollTop;
 Object.assign(state,extra); $('#selection-action').hidden=true; closeSheet();
 let hash=route;
 if(route==='chat')hash+='/' + state.chat;
 if(['device','agents','models','connection'].includes(route))hash+='/' + state.device;
 if(route==='connection')hash+='/' + state.connectionIndex;
 if(location.hash==='#'+hash){readRoute();return;} location.hash=hash;
}
function readRoute(){
 if(state.currentHash)state.scroll[state.currentHash]=$('#main').scrollTop;
 state.currentHash=location.hash || '#nav';
 const [route,id,index]=(location.hash.slice(1)||'nav').split('/');
 state.route=['nav','chat','settings','device','agents','models','connection'].includes(route)?route:'nav';
 if(state.route==='chat'){
   if(![...state.tasks,...state.leaders].some(x=>x.id===id)){navigate('nav');return;}
   state.chat=id;state.lastChat=id;state.device=chatLeader(id).device;
   const task=state.tasks.find(t=>t.id===id);if(task)task.unread=0;
   if(!state.messages[id]){const l=chatLeader(id);state.messages[id]=[{name:l.name,avatar:l.avatar,device:l.device,text:id==='offline'?'已同步的对话可以离线阅读。恢复连接后，再继续发送草稿。':'有什么想继续推进的？可以直接在这里告诉我。',time:'10:20'}];}
 }
 if(['device','agents','models','connection'].includes(state.route)){state.device=state.devices.some(d=>d.id===id)?id:'mini1';state.connectionIndex=Number(index)||0;}
 render();$('#main').scrollTop=state.scroll[location.hash]||0;
}
function render(){
 $('#app').classList.toggle('nav-mode',state.route==='nav');$('#main').dataset.route=state.route;$('#footer').innerHTML='';
 if(state.route==='nav')renderNav();else if(state.route==='chat')renderChat();else renderSettings();
}
let brandIntroShown=false;
function renderNav(){
 const brandAnimation=brandIntroShown?'':' brand-entry';brandIntroShown=true;
 $('#header').innerHTML=`<div class="brand${brandAnimation}"><img class="brand-mark" src="assets/mark.svg" alt=""><img class="brand-wordmark" src="assets/brand/zork-wordmark.svg" alt="Zork"></div>`;
 $('#main').innerHTML=state.devices.map(d=>`<section class="device-group" aria-label="${d.id}"><button class="nav-row device" data-action="fold-device" data-id="${d.id}" aria-expanded="${!state.collapsed.has(d.id)}">${icon('node')}<span class="title">${d.id}</span><span class="online ${d.online?'':'offline'}">${d.online?'在线':'离线'}</span></button>${state.collapsed.has(d.id)?'':state.leaders.filter(l=>l.device===d.id).map(l=>`<div class="nav-row leader"><button class="leader-open" data-action="chat" data-id="${l.id}">${avatar(l.avatar)}<span class="title">${l.name}</span></button><button class="leader-toggle" aria-label="${state.collapsed.has(l.id)?'展开':'收起'}${l.name}的任务" aria-expanded="${!state.collapsed.has(l.id)}" data-action="fold-leader" data-id="${l.id}">${icon('chevron-down','chevron '+(state.collapsed.has(l.id)?'closed':''))}</button></div>${state.collapsed.has(l.id)?'':state.tasks.filter(t=>t.leader===l.id).map(t=>`<button class="nav-row task" data-action="chat" data-id="${t.id}"><span class="title">${t.name}</span>${t.unread?`<span class="count" aria-label="${t.unread} 条未读">${t.unread}</span>`:''}</button>`).join('')}`).join('')}</section>`).join('');
 $('#footer').innerHTML=`<div class="nav-footer"><button data-action="add-device">${icon('plus')}添加设备</button><button data-action="settings">${icon('settings')}设置</button></div>`;
}
function fileMarkup(file){return `<a class="file-card" href="mobile-notes.md" download="zork-mobile-notes.md">${icon('result')}<span><span class="file-name">${escapeHTML(file)}</span><small>设计说明 · Markdown</small></span>${icon('download','download')}</a>`;}
function messageMarkup(m){
 const content=`${m.comments?.map(c=>`<div class="sent-comment"><blockquote>${escapeHTML(c.quote)}</blockquote>${escapeHTML(c.text)}</div>`).join('')||''}${m.text?`<div class="message-text">${escapeHTML(m.text)}</div>`:''}${m.file?fileMarkup(m.file):''}${m.attachments?.map(f=>`<a class="file-card" href="${f.url}" download="${escapeHTML(f.name)}">${icon('paperclip')}<span><span class="file-name">${escapeHTML(f.name)}</span><small>${Math.max(1,Math.round(f.size/1024))} KB · 本地附件</small></span>${icon('download','download')}</a>`).join('')||''}`;
 if(m.own)return `<article class="message own"><div class="bubble">${content}</div><span class="timestamp">${m.time}${m.local?' · 本地消息':''}</span></article>`;
 return `<article class="message agent-message">${avatar(m.avatar)}<div><div class="message-meta"><strong>${m.name}</strong><span>${m.device} · ${m.time}</span></div>${content}</div></article>`;
}
function renderChat(){
 const l=chatLeader(state.chat);const roster=state.chat==='brand'?['fox','cat','panda']:[l.avatar];
 $('#header').innerHTML=`<button class="icon-button" data-action="nav" aria-label="返回对话列表">${icon('arrow-left')}</button><div class="roster" aria-label="对话成员">${roster.map(a=>`<button data-action="member" data-id="${a}" aria-label="查看${a==='fox'?'产品 Leader':a==='cat'?'设计 Worker':a==='panda'?'插画 Worker':l.name}">${avatar(a)}</button>`).join('')}</div><button class="header-device" data-action="device" data-id="${l.device}" aria-label="查看 ${l.device} 设置">${icon('node')}${l.device}</button>`;
 $('#main').innerHTML=`<div class="chat-log"><div class="date-divider">今天</div>${state.messages[state.chat].map(messageMarkup).join('')}</div>`;
 $('#footer').innerHTML=`<div class="composer-area"><div id="pending-comments"></div><form id="composer-form" class="composer"><textarea id="message-input" rows="1" placeholder="补充想法…" aria-label="消息" enterkeyhint="enter">${escapeHTML(state.drafts[state.chat]||'')}</textarea><div id="pending-attachments" class="attachments"></div><div class="composer-actions"><button type="button" class="icon-button" data-action="attach" aria-label="添加附件">${icon('paperclip')}</button><button type="submit" class="send" aria-label="发送消息">${icon('arrow-up')}</button></div></form></div>`;
 renderPending();resizeInput();
}
function renderPending(){
 const comments=currentComments();$('#pending-comments').innerHTML=comments.length?`<div class="comment-tray"><div class="tray-header"><span>待发送评论 · ${comments.length}</span><span>点按编辑</span></div>${comments.map((c,i)=>`<div class="pending-comment"><button class="comment-edit" data-action="edit-comment" data-index="${i}"><blockquote>${escapeHTML(c.quote)}</blockquote>${escapeHTML(c.text)}</button><button class="remove-comment" data-action="remove-comment" data-index="${i}" aria-label="移除第 ${i+1} 条评论">${icon('x')}</button></div>`).join('')}</div>`:'';
 $('#pending-attachments').innerHTML=currentAttachments().map((f,i)=>`<span class="attachment-chip"><span>${escapeHTML(f.name)}</span><button type="button" data-action="remove-attachment" data-index="${i}" aria-label="移除附件">${icon('x')}</button></span>`).join('');
 $('.send').disabled=!(comments.length || currentAttachments().length || (state.drafts[state.chat]||'').trim());
}
function resizeInput(){const el=$('#message-input');if(el){el.style.height='28px';el.style.height=Math.min(128,el.scrollHeight)+'px';}}
function backHeader(title,action='settings',extra=''){return `<button class="icon-button" data-action="${action}" aria-label="返回">${icon('arrow-left')}</button><span class="toolbar-title">${title}</span>${extra}`;}
function deviceRow(d){return `<button class="setting-row" data-action="device" data-id="${d.id}">${icon('node')}<span>${d.id}</span><span class="online">在线</span>${arrow()}</button>`;}
function renderSettings(){
 const d=currentDevice();let html='';
 if(state.route==='settings'){
  $('#header').innerHTML=backHeader('设置','nav');
  html=`<div class="settings-body settings-home"><h1>设置</h1><p class="section-label">客户端设置</p><p class="empty-note">目前没有需要配置的客户端选项。</p><p class="section-label">设备设置</p><div class="setting-group device-list">${state.devices.map(deviceRow).join('')}</div><button class="secondary-button" data-action="add-device">${icon('plus')}添加设备</button><p class="demo-note">当前为交互原型，所有设备与消息均为演示数据。</p></div>`;
 }else if(state.route==='device'){
  const returnLabel=state.deviceReturn?.route==='chat'?'返回对话':'设置';
  $('#header').innerHTML=`<button class="header-back" data-action="device-return" aria-label="${returnLabel}">${icon('arrow-left')}<span>${returnLabel}</span></button>`;
  html=`<div class="settings-body"><div class="device-heading">${icon('node')}<h1>${d.id}</h1></div><span class="online">在线</span><p class="section-label">设备</p><div class="setting-group"><div class="setting-row"><span>Gateway 版本</span><span class="value">待获取</span></div><button class="setting-row" data-action="check-update"><span>检查更新</span>${arrow()}</button></div><p class="hint">版本信息尚未获取</p><div class="setting-group"><button class="setting-row" data-action="toggle" data-key="background" role="switch" aria-checked="${d.background}"><span>后台运行</span><span class="toggle ${d.background?'on':''}" aria-hidden="true"></span></button><button class="setting-row" data-action="toggle" data-key="startup" role="switch" aria-checked="${d.startup}"><span>登录后启动</span><span class="toggle ${d.startup?'on':''}" aria-hidden="true"></span></button></div><p class="section-label">Agent 与模型</p><div class="setting-group"><button class="setting-row" data-action="agents">${avatar(chatLeader(d.id==='mini1'?'product':'research').avatar)}<span>Agent</span><span class="value">${state.agents[d.id].length}</span>${arrow()}</button><button class="setting-row" data-action="models">${icon('mesh')}<span>模型连接</span><span class="value">${state.connections[d.id].length}</span>${arrow()}</button></div>${assistance(d.id)}</div>`;
 }else if(state.route==='agents'){
  $('#header').innerHTML=backHeader('Agent','device-back',`<button class="page-action" data-action="add-agent">添加</button>`);
  html=`<div class="settings-body"><p class="muted">${d.id}</p>${['Leader','Worker'].map(kind=>`<p class="section-label">${kind}</p><div class="setting-group">${state.agents[d.id].map((a,i)=>a.kind===kind?`<button class="setting-row" data-action="edit-agent" data-index="${i}">${avatar(a.avatar)}<span>${escapeHTML(a.name)}<small class="subtext">${escapeHTML(a.model||'未配置模型')}</small></span>${arrow()}</button>`:'').join('')}</div>`).join('')}<p class="hint">Agent 只能使用本设备的模型连接。</p></div>`;
 }else if(state.route==='models'){
  $('#header').innerHTML=backHeader('模型连接','device-back',`<button class="page-action" data-action="add-connection">添加</button>`);
  html=`<div class="settings-body"><p class="muted">${d.id}</p><p class="hint">连接保存提供商账号与接入方式，模型在连接详情中管理。</p><div class="setting-group">${state.connections[d.id].map((c,i)=>`<button class="setting-row" data-action="connection" data-index="${i}">${icon('mesh')}<span>${escapeHTML(c.name)}<small class="subtext">${c.kind} · ${c.models.length?c.models.length+' 个模型':'待配置模型'}</small></span>${arrow()}</button>`).join('')}</div>${state.connections[d.id].length?'':`<p class="empty-note">还没有模型连接。添加后即可为 ${d.id} 的 Agent 选择模型。</p>`}${assistance(d.id)}</div>`;
 }else{
  const c=state.connections[d.id][state.connectionIndex];if(!c){navigate('models');return;}
  $('#header').innerHTML=backHeader('连接详情','models');
  html=`<div class="settings-body"><h2>${escapeHTML(c.name)}</h2><p class="hint">${d.id} · ${c.kind} · 未验证</p><p class="section-label">可用模型</p><div class="setting-group">${c.models.map(m=>`<div class="setting-row"><span>${escapeHTML(m)}</span></div>`).join('')}</div>${c.models.length?'':'<p class="empty-note">没有预设模型。连接服务后获取，或手动添加模型 ID。</p>'}<button class="secondary-button" data-action="fetch-models">获取可用模型</button><button class="primary-button" data-action="add-model">手动添加模型</button></div>`;
 }
 $('#main').innerHTML=html;
}
function assistance(device){return `<div class="assistance"><h3>也可以交给 Leader</h3><p>选择一位伙伴，继续讨论设备配置。</p><div class="leader-options">${state.leaders.map(l=>`<button class="leader-option" data-action="assist" data-id="${l.id}" data-device="${device}">${avatar(l.avatar)}<span>${l.name.split(' ')[0]}</span></button>`).join('')}</div></div>`;}
function openSheet(title,body){$('#selection-action').hidden=true;$('#sheet-content').innerHTML=`<div class="sheet-header"><h2 id="sheet-title">${title}</h2><button class="icon-button" data-action="close-sheet" aria-label="关闭">${icon('x')}</button></div>${body}`;if(!$('#sheet').open)$('#sheet').showModal();}
function closeSheet(){if($('#sheet').open)$('#sheet').close();}
function commentSheet(index=-1){const existing=currentComments()[index];const quote=existing?.quote||state.selectedText;if(!quote)return;openSheet(index<0?'评论所选片段':'编辑评论',`<blockquote class="sheet-quote">${escapeHTML(quote)}</blockquote><form id="comment-form" data-index="${index}"><label class="field">你的评论<textarea name="comment" required placeholder="对这段内容有什么想法？">${escapeHTML(existing?.text||'')}</textarea></label><button class="primary-button">加入待发送评论</button><p class="hint">可以继续添加其他评论，最后和消息一起发送。</p></form>`);state.commentQuote=quote;}
function addDevice(){openSheet('添加设备',`<p class="sheet-copy">选择一位 Leader 协助接入。请求会先放入对话草稿，确认后再发送。</p><div class="leader-options">${state.leaders.map(l=>`<button class="leader-option" data-action="assist-add" data-id="${l.id}">${avatar(l.avatar)}<span>${l.name.split(' ')[0]}</span></button>`).join('')}</div><p class="section-label">手动接入</p><p class="sheet-copy">也可以在新设备上执行加入命令。</p><button class="secondary-button" data-action="join-command">查看接入方式</button>`);}
function addConnection(){openSheet('添加模型连接',`<p class="sheet-copy">归属设备：${state.device}</p><div class="segmented"><button class="${state.connectKind==='订阅账号'?'active':''}" data-action="connection-kind" data-kind="订阅账号">订阅账号</button><button class="${state.connectKind==='API 接入'?'active':''}" data-action="connection-kind" data-kind="API 接入">API 接入</button></div><form id="connection-form"><label class="field">供应商<select name="provider">${(state.connectKind==='订阅账号'?['OpenAI','Anthropic']:['OpenAI','Anthropic','Google','兼容接口']).map(p=>`<option>${p}</option>`).join('')}</select></label><label class="field">连接名称<input name="name" placeholder="例如：个人订阅" maxlength="60"></label><p class="hint">原型只保存演示连接，不收集 API Key 或执行账号授权。</p><button class="primary-button">保存演示连接</button></form>`);}
function editAgent(index){
 const a=state.agents[state.device][index];const choices=state.connections[state.device].flatMap((c,ci)=>c.models.map(m=>({label:c.name+' / '+m,value:ci+':'+m})));
 openSheet('配置 Agent',`<form id="agent-form" data-index="${index}"><label class="field">名称<input name="name" value="${escapeHTML(a.name)}" required maxlength="60"></label><label class="field">模型<select name="model"><option value="">未配置模型</option>${choices.map(m=>`<option value="${escapeHTML(m.value)}" ${a.modelId===m.value?'selected':''}>${escapeHTML(m.label)}</option>`).join('')}</select></label>${choices.length?'':'<p class="hint">请先在本设备的模型连接中添加可用模型。</p>'}<button class="primary-button">保存</button></form>`);
}
function sendMessage(){
 const input=$('#message-input');const text=input.value.trim();const comments=currentComments(),attachments=currentAttachments();if(!text&&!comments.length&&!attachments.length)return;
 state.messages[state.chat].push({own:true,text,comments:[...comments],attachments:[...attachments],time:timeNow(),local:true});
 state.drafts[state.chat]='';state.comments[state.chat]=[];state.attachments[state.chat]=[];
 renderChat();$('#main').scrollTop=$('#main').scrollHeight;toast('已添加到本地对话，未发送给真实 Agent');
}
function handleAction(button){
 const a=button.dataset.action,id=button.dataset.id,index=Number(button.dataset.index);
 if(a==='nav'||a==='settings'||a==='agents'||a==='models')navigate(a);
 else if(a==='chat')navigate('chat',{chat:id});
 else if(a==='fold-device'||a==='fold-leader'){state.collapsed.has(id)?state.collapsed.delete(id):state.collapsed.add(id);renderNav();}
 else if(a==='device'){
  state.deviceReturn=state.route==='chat'?{route:'chat',chat:state.chat}:{route:'settings'};
  navigate('device',{device:id});
 }
 else if(a==='device-return'){
  const origin=state.deviceReturn||{route:'settings'};navigate(origin.route,origin.chat?{chat:origin.chat}:{});
 }
 else if(a==='device-back')navigate('device');
 else if(a==='toggle'){const d=currentDevice();d[button.dataset.key]=!d[button.dataset.key];renderSettings();toast('已更新本地演示设置');}
 else if(a==='check-update')toast('原型未连接真实设备，无法检查版本更新');
 else if(a==='add-device')addDevice();
 else if(a==='close-sheet')closeSheet();
 else if(a==='member'){const member={fox:['产品 Leader','mini1'],cat:['设计 Worker','mini2'],panda:['插画 Worker','mini2'],dog:['工程 Leader','mini1'],owl:['研究 Leader','mini2']}[id];openSheet('对话成员',`<div class="member-detail">${avatar(id)}<div><h3>${member[0]}</h3><p>${member[1]}</p></div></div><p class="sheet-copy">这位伙伴参与当前对话。消息中的头像和这里保持一致。</p>`);}
 else if(a==='attach'){$('#attachment-input').dataset.chat=state.chat;$('#attachment-input').click();}
 else if(a==='edit-comment')commentSheet(index);
 else if(a==='remove-comment'){currentComments().splice(index,1);renderPending();}
 else if(a==='remove-attachment'){const [f]=currentAttachments().splice(index,1);URL.revokeObjectURL(f.url);renderPending();}
 else if(a==='assist'||a==='assist-add'){
  const prompt=a==='assist-add'?'我想添加一台新设备，请帮我准备接入步骤。':`帮我看看 ${button.dataset.device} 的设备配置。`;
  state.drafts[id]=[state.drafts[id],prompt].filter(Boolean).join('\n');navigate('chat',{chat:id});
 }
 else if(a==='join-command'){openSheet('手动接入',`<p class="sheet-copy">实际接入需要由已有设备生成邀请，再按邀请提供的命令安装并加入。</p><p class="hint">原型无法生成有效邀请。你可以先让 Leader 准备接入步骤。</p><button class="primary-button" data-action="add-device">选择 Leader 协助</button>`);}
 else if(a==='connection')navigate('connection',{connectionIndex:index});
 else if(a==='add-connection'){state.connectKind='订阅账号';addConnection();}
 else if(a==='connection-kind'){state.connectKind=button.dataset.kind;addConnection();}
 else if(a==='fetch-models')toast('原型未连接模型服务，请手动添加演示模型');
 else if(a==='add-model')openSheet('手动添加模型',`<form id="model-form"><label class="field">模型 ID<input name="model" placeholder="填写提供商的模型 ID" required maxlength="120"></label><p id="model-error" class="error-text"></p><button class="primary-button">添加模型</button></form>`);
 else if(a==='edit-agent')editAgent(index);
 else if(a==='add-agent')openSheet('添加 Agent',`<form id="new-agent-form"><label class="field">名称<input name="name" required placeholder="例如：写作 Worker" maxlength="60"></label><label class="field">角色<select name="kind"><option>Worker</option><option>Leader</option></select></label><button class="primary-button">添加演示 Agent</button><p class="hint">仅添加配置记录，不启动真实 Agent。</p></form>`);
}
document.addEventListener('click',e=>{const button=e.target.closest('[data-action]');if(button){handleAction(button);}});
document.addEventListener('submit',e=>{
 const f=e.target;e.preventDefault();const data=new FormData(f);
 if(f.id==='composer-form'){sendMessage();return;}
 if(f.id==='comment-form'){
  const text=String(data.get('comment')).trim();if(!text)return;const c={quote:state.commentQuote,text},i=Number(f.dataset.index);if(i>=0)currentComments()[i]=c;else currentComments().push(c);closeSheet();renderPending();return;
 }
 if(f.id==='connection-form'){
  const provider=String(data.get('provider')),name=String(data.get('name')).trim()||provider+' '+state.connectKind;
  state.connections[state.device].push({name,provider,kind:state.connectKind,models:[]});closeSheet();navigate('connection',{connectionIndex:state.connections[state.device].length-1});return;
 }
 if(f.id==='model-form'){
  const model=String(data.get('model')).trim(),c=state.connections[state.device][state.connectionIndex];if(!model)return;
  if(c.models.includes(model)){$('#model-error').textContent='这个模型已经在连接中。';return;}
  c.models.push(model);closeSheet();renderSettings();return;
 }
 if(f.id==='agent-form'){
  const a=state.agents[state.device][Number(f.dataset.index)],name=String(data.get('name')).trim();if(!name)return;
  a.name=name;a.modelId=String(data.get('model'));a.model=a.modelId?a.modelId.slice(a.modelId.indexOf(':')+1):'';closeSheet();renderSettings();return;
 }
 if(f.id==='new-agent-form'){
  const name=String(data.get('name')).trim();if(!name)return;const kind=String(data.get('kind'));state.agents[state.device].push({name,kind,avatar:kind==='Leader'?'fox':'bunny'});closeSheet();renderSettings();toast('已添加演示配置，未启动 Agent');
 }
});
document.addEventListener('input',e=>{if(e.target.id==='message-input'){state.drafts[state.chat]=e.target.value;resizeInput();renderPending();}});
$('#attachment-input').addEventListener('change',e=>{
 const chat=e.target.dataset.chat;const files=Array.from(e.target.files);state.attachments[chat] ||= [];
 for(const f of files)state.attachments[chat].push({name:f.name,size:f.size,url:URL.createObjectURL(f)});
 e.target.value='';if(state.route==='chat'&&state.chat===chat)renderPending();
});
function handleSelection(){
 if(state.route!=='chat'||$('#sheet').open)return;const sel=window.getSelection();const node=sel?.anchorNode?.parentElement;const end=sel?.focusNode?.parentElement;const text=sel?.toString().trim();
 if(text&&node?.closest('.message-text')&&end?.closest('.message-text')){state.selectedText=text;$('#selection-action').style.bottom=($('#footer').offsetHeight+12)+'px';$('#selection-action').hidden=false;}
 else if(!text){$('#selection-action').hidden=true;}
}
document.addEventListener('selectionchange',()=>{clearTimeout(handleSelection.timer);handleSelection.timer=setTimeout(handleSelection,140);});
$('#selection-action').addEventListener('pointerdown',e=>e.preventDefault());
$('#selection-action').addEventListener('click',()=>commentSheet());
$('#sheet').addEventListener('click',e=>{if(e.target===$('#sheet')){const r=$('#sheet').getBoundingClientRect();if(e.clientX<r.left||e.clientX>r.right||e.clientY<r.top||e.clientY>r.bottom)closeSheet();}});
window.addEventListener('hashchange',readRoute);
function viewport(){if(window.innerWidth<600){document.documentElement.style.setProperty('--app-height',(window.visualViewport?.height||window.innerHeight)+'px');}else document.documentElement.style.removeProperty('--app-height');}
window.visualViewport?.addEventListener('resize',viewport);window.addEventListener('resize',viewport);viewport();readRoute();

// One pressed surface per navigation row; scrolling cancels touch feedback.
let pressedNavigation = null;
function clearNavigationPress(){
 if(pressedNavigation)pressedNavigation.row.classList.remove('is-pressed');
 pressedNavigation=null;
}
document.addEventListener('pointerdown',event=>{
 clearNavigationPress();
 if(event.button!==0)return;
 const row=event.target.closest('.nav-row,.nav-footer>button,.setting-group>button.setting-row,.header-device,.header-back');
 if(!row)return;
 pressedNavigation={row,x:event.clientX,y:event.clientY,id:event.pointerId};
 row.classList.add('is-pressed');
});
document.addEventListener('pointermove',event=>{
 if(pressedNavigation && event.pointerId===pressedNavigation.id &&
   Math.hypot(event.clientX-pressedNavigation.x,event.clientY-pressedNavigation.y)>10)clearNavigationPress();
},{passive:true});
document.addEventListener('pointerup',clearNavigationPress);
document.addEventListener('pointercancel',clearNavigationPress);
document.addEventListener('scroll',clearNavigationPress,{capture:true,passive:true});
window.addEventListener('blur',clearNavigationPress);
window.addEventListener('hashchange',clearNavigationPress);
