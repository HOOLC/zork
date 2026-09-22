/* Local interface preferences; device/model operations are handed to a Leader. */
'use strict';
const preferences={font:'13',timestamps:true,send:'enter'};
let settingsScope='device',settingsDevice='mini1',settingsDeviceTab='overview',settingsOpen=false;
let profiles=[];
const devicePreferences={mini1:{background:true,atLogin:false},mini2:{background:true,atLogin:false}};
const deviceUpdateChecks={};
function openSettings(tab='overview',node=chat().node){
 settingsScope='device';settingsDevice=devices[node]?node:'mini1';settingsDeviceTab=['agents','models'].includes(tab)?tab:'overview';settingsOpen=true;
 if(typeof closeSelectionReply==='function')closeSelectionReply();
 document.querySelector('.conversation-main').hidden=true;$('context-panel').hidden=true;$('settings-view').hidden=false;
 document.body.classList.remove('mobile-list');document.body.classList.add('settings-mode');document.querySelector('[data-action="settings"]').classList.add('selected');
 renderSettings();$('settings-view').focus();
}
function closeSettings(){
 document.body.classList.remove('settings-mode');settingsOpen=false;$('settings-view').hidden=true;document.querySelector('.conversation-main').hidden=false;
 $('context-panel').hidden=!panel;document.querySelector('[data-action="settings"]').classList.remove('selected');
}
function addDeviceDialog(){
 modal('添加设备',`<div class="add-device-dialog"><p class="add-device-intro">在新设备上执行加入命令，连接后它会出现在设备列表中。</p><div class="add-device-leaders"><b>交给 Leader</b><p>点击头像，选择由谁协助添加设备。</p><div class="add-device-leader-row" role="group" aria-label="选择负责添加设备的 Leader">${Object.entries(people).filter(([,p])=>p.role==='Leader').map(([id,p])=>`<button class="add-device-leader" data-action="settings-talk" data-topic="add-device" data-leader="${id}" aria-label="交给${esc(p.name)}添加设备" title="${esc(p.name)} · ${p.node}">${av(p.avatar)}</button>`).join('')}</div></div><div class="add-device-manual"><h3>手动添加</h3><p>获取命令后，复制到新设备的终端执行。</p><button class="button primary" data-action="get-join-command">获取加入命令</button><div id="join-command-result" hidden></div></div></div>`);
}
function showJoinCommand(){
 const command="curl -fsSL https://github.com/HOOLC/zork/releases/download/v0.1.30/install.sh | sh -s -- --version 0.1.30 -- mesh join '<invitation>'";
 const el=$('join-command-result');if(!el)return;
 el.hidden=false;el.innerHTML=`<div class="join-command-header"><span>在新设备执行</span><button class="settings-text-button" data-action="copy-join-command">复制命令模板</button></div><textarea id="join-command" readonly aria-label="新设备加入命令模板" spellcheck="false"></textarea><p class="join-command-note" role="status">样稿显示命令模板；接入真实节点后，这里会生成包含有效邀请的命令。</p>`;
 $('join-command').value=command;
 document.querySelector('[data-action="get-join-command"]').hidden=true;
}
async function copyJoinCommand(button){
 const field=$('join-command');if(!field)return;let copied=false;
 try{await navigator.clipboard.writeText(field.value);copied=true;}catch{
   field.focus();field.select();try{copied=document.execCommand('copy');}catch{}
 }
 button.textContent=copied?'已复制模板':'请按 ⌘ / Ctrl C 复制';
}
function editAgentDialog(id=null){
 const p=id?people[id]:null,node=p?.node||settingsDevice;
 const available=profiles.filter(profile=>profile.node===node),selected=available.find(profile=>profile.id===p?.modelConnectionId);
 modal(p?'设置 Agent':'添加 Agent',`<form id="agent-settings-form" data-device="${node}" data-agent="${id||''}"><p class="model-form-note">${icon('node')} ${node} · Agent 在这台设备运行</p><label class="field"><span>名称</span><input name="name" value="${esc(p?.name||'')}" maxlength="60" required placeholder="Agent 名称"></label><label class="field"><span>角色</span>${p?`<input value="${p.role}" readonly>`:'<select name="role"><option value="Leader">Leader</option><option value="Worker">Worker</option></select>'}</label><label class="field"><span>模型连接 / Profile</span><select id="agent-profile" name="profile"><option value="">暂不配置</option>${available.map(profile=>`<option value="${profile.id}" ${selected?.id===profile.id?'selected':''}>${esc(profile.name)}</option>`).join('')}</select></label><label class="field"><span>模型</span><select id="agent-model" name="model" ${selected?.models.length?'':'disabled'}>${selected?.models.length?selected.models.map(model=>`<option value="${esc(model.id)}" ${p?.modelId===model.id?'selected':''}>${esc(model.id)}</option>`).join(''):`<option value="">${selected?'此连接尚未添加模型':'先选择模型连接'}</option>`}</select></label>${available.length?'':'<p class="model-form-note">这台设备还没有 Profile，请先在“模型连接”中添加账号。</p>'}<p id="agent-settings-error" class="profile-error" role="alert" hidden></p><p class="model-form-note">当前修改仅用于样稿预览，不会启动真实 Agent。</p><div class="modal-actions"><button class="button" type="button" data-action="close-dialog">取消</button><button class="button primary" type="submit">${p?'保存':'添加 Agent'}</button></div></form>`);
}

function renderSettings(){
 const device=devices[settingsDevice],agents=Object.entries(people).filter(([,p])=>p.node===settingsDevice),models=profiles.filter(model=>model.node===settingsDevice);
 const agentRow=([id,p])=>{const profile=models.find(profile=>profile.id===p.modelConnectionId),model=profile?.models.find(model=>model.id===p.modelId);const label=profile?profile.name+' / '+(model?.id||'未选模型'):'未配置';return `<button class="setting-row settings-agent-row" data-action="edit-agent" data-agent="${id}" aria-label="设置 ${esc(p.name)}，${esc(label)}">${av(p.avatar)}<b class="settings-agent-name">${esc(p.name)}</b><span class="agent-model-label ${profile?'':'unconfigured'}" title="${esc(label)}">${esc(label)}</span></button>`;};
 let content='',title='客户端设置',headerAction='';
 if(settingsScope==='client')content='<p class="settings-caption">仅影响当前客户端。Agent 和模型按设备独立配置。</p><div class="client-settings-empty"><p>暂未规划客户端专属设置。</p><span>设备上的配置可从左侧进入。</span></div>';
 else{
  const overview=settingsDeviceTab==='overview';
  title=overview?device.name:device.name+' / '+(settingsDeviceTab==='agents'?'Agent':'模型连接');
  if(overview){
   const pref=devicePreferences[settingsDevice];
   content=`<h2 class="device-overview-heading">Gateway</h2><div class="device-overview-group"><div class="device-option-row"><div><b>当前版本</b><small>0.1.30 · 示例版本</small></div><button class="button" data-action="device-check-update" data-device="${settingsDevice}">检查更新</button></div>${deviceUpdateChecks[settingsDevice]?'<p class="device-update-feedback" role="status">尚未连接真实设备，无法查询版本更新。当前完整版本升级需要手动执行。</p>':''}<div class="device-option-row"><div><b>运行状态</b><small>${pref.background?'后台服务管理':'随客户端运行'}</small></div><span class="setting-device-status"><i></i>运行中 · 示例</span></div></div><h2 class="device-overview-heading">运行偏好</h2><div class="device-overview-group"><div class="device-option-row"><div><b id="background-label">退出客户端后保持运行</b><small>由设备上的后台服务接管 Gateway</small></div><button class="device-setting-toggle" role="switch" aria-labelledby="background-label" aria-checked="${pref.background}" data-action="device-preference" data-device="${settingsDevice}" data-key="background">${pref.background?'已开启':'已关闭'}</button></div><div class="device-option-row"><div><b id="at-login-label">登录系统后自动启动</b><small>${pref.background?'登录这台设备时启动 Gateway':'需要先开启后台运行'}</small></div><button class="device-setting-toggle" role="switch" aria-labelledby="at-login-label" aria-checked="${pref.atLogin}" data-action="device-preference" data-device="${settingsDevice}" data-key="atLogin" ${pref.background?'':'disabled'}>${pref.atLogin?'已开启':'已关闭'}</button></div></div><p class="device-overview-note">本地样稿：版本、运行状态和偏好仅作演示，不会更新或重启设备。</p>`;
  }else{
   headerAction=`<button class="button settings-page-add" data-action="${settingsDeviceTab==='agents'?'add-agent':'add-profile'}">${icon('plus')}${settingsDeviceTab==='agents'?'添加 Agent':'添加连接'}</button>`;
   if(settingsDeviceTab==='agents')content='<div class="agent-table-heading"><span>名称</span><span>Profile / 模型</span></div>'+(['Leader','Worker'].map(role=>{const rows=agents.filter(([,p])=>p.role===role);return rows.length?`<h2 class="device-agent-group-title">${role}</h2><div class="settings-group">${rows.map(agentRow).join('')}</div>`:'';}).join('')||'<p class="model-empty">这台设备还没有 Agent。</p>');
   else content=`<p class="profile-list-note">连接提供商账号（Profile），再为 Agent 选择其中的模型。</p><div class="settings-group">${models.length?models.map(profile=>`<button class="setting-row profile-list-row" data-action="view-profile" data-profile="${profile.id}">${providerLogo(profile.provider)}<div class="setting-device-copy"><b>${esc(profile.name)}</b><small>${esc(profileCatalog.find(p=>p.id===profile.provider)?.label||profile.provider)} · ${esc(profileCatalog.find(p=>p.id===profile.provider)?.billing.find(b=>b.id===profile.billing)?.label||profile.billing)}</small></div><span class="profile-model-count">${profile.models.length?`${profile.models.length} 个模型`:'待配置模型'}</span><span class="model-unverified">未验证</span></button>`).join(''):'<p class="model-empty">这台设备还没有模型连接</p>'}</div>`;
  }
  const leaders=agents.filter(([,p])=>p.role==='Leader');
  if(leaders.length)content+=`<div class="device-settings-agent-help"><span>交给 Leader</span><div class="add-device-leader-row" role="group" aria-label="选择 ${device.name} 的 Leader">${leaders.map(([id,p])=>`<button class="add-device-leader" data-action="settings-talk" data-topic="${settingsDeviceTab==='overview'?'device':settingsDeviceTab==='models'?'model':'agent'}" data-leader="${id}" title="${esc(p.name)} · ${p.node}" aria-label="交给${esc(p.name)}配置">${av(p.avatar)}</button>`).join('')}</div></div>`;
 }
 const deviceNavigation=Object.entries(devices).map(([id,d])=>{
  const active=settingsScope==='device'&&id===settingsDevice;
  return `<div class="settings-device-tree"><button class="settings-device-nav" data-action="device-settings" data-device="${id}" ${active&&settingsDeviceTab==='overview'?'aria-current="page"':''}>${icon('node')}<span class="settings-device-name">${d.name}</span><i aria-label="${d.online?'在线':'离线'}"></i></button><div class="settings-device-children" aria-label="${d.name} 设置">${[['agents','Agent'],['models','模型连接']].map(([page,label])=>`<button data-action="device-settings-page" data-device="${id}" data-page="${page}" ${active&&settingsDeviceTab===page?'aria-current="page"':''}><span>${label}</span><small>${page==='agents'?Object.values(people).filter(p=>p.node===id).length:profiles.filter(p=>p.node===id).length}</small></button>`).join('')}</div></div>`;
 }).join('');
 $('settings-view').innerHTML=`<aside class="settings-navigation"><button class="settings-back" data-action="close-settings">${icon('arrow-left')}返回对话</button><nav aria-label="设置范围"><button data-action="client-settings" ${settingsScope==='client'?'aria-current="page"':''}>客户端设置</button><div class="settings-nav-label">设备设置</div>${deviceNavigation}<button class="settings-add-device" data-action="add-device">${icon('plus')}添加设备</button></nav><div class="settings-nav-footer">Zork<span>本地样稿</span></div></aside><div class="settings-pane"><div class="settings-content"><header class="settings-header"><h1 id="settings-title">${title}</h1>${settingsScope==='device'&&settingsDeviceTab==='overview'?`<span class="settings-device-description">${device.description}</span><span class="setting-device-status"><i></i>在线</span>`:''}${headerAction}</header><div class="settings-section">${content}</div></div></div>`;
}
document.addEventListener('click',e=>{
 const b=e.target.closest('[data-action]');if(!b)return;
 if(b.dataset.action==='settings')openSettings();
 else if(b.dataset.action==='close-settings'){closeSettings();document.querySelector('[data-action="settings"]').focus();}
 else if(b.dataset.action==='client-settings'){settingsScope='client';renderSettings();}
 else if(b.dataset.action==='device-settings'){settingsScope='device';settingsDeviceTab='overview';settingsDevice=b.dataset.device;renderSettings();}
 else if(b.dataset.action==='device-settings-page'){settingsScope='device';settingsDevice=b.dataset.device;settingsDeviceTab=b.dataset.page;renderSettings();}
 else if(b.dataset.action==='device-check-update'){deviceUpdateChecks[b.dataset.device]=true;renderSettings();}
 else if(b.dataset.action==='device-preference'){const pref=devicePreferences[b.dataset.device],key=b.dataset.key;if(!pref||!['background','atLogin'].includes(key)||(key==='atLogin'&&!pref.background))return;pref[key]=!pref[key];if(!pref.background)pref.atLogin=false;renderSettings();}
 else if(b.dataset.action==='add-device')addDeviceDialog();
 else if(b.dataset.action==='add-profile')addProfileDialog();
 else if(b.dataset.action==='add-agent')editAgentDialog();
 else if(b.dataset.action==='edit-agent')editAgentDialog(b.dataset.agent);
 else if(b.dataset.action==='get-join-command')showJoinCommand();
 else if(b.dataset.action==='copy-join-command')void copyJoinCommand(b);
 else if(b.dataset.action==='settings-talk'){
   const leaderId=b.dataset.leader||'engineering',c=leaderConversation(leaderId);
   const request=b.dataset.topic==='add-device'?'我想添加一台设备，请带我完成连接。':b.dataset.topic==='model'?'帮我检查这台设备的模型配置，我想调整使用的模型。':b.dataset.topic==='agent'?'帮我调整这台设备上的 Agent 配置。':'帮我检查设备连接，我想调整工作的执行位置。';
   drafts[c.id]=drafts[c.id]?drafts[c.id]+'\n'+request:request;selectChat(c.id);document.querySelector('#message-form textarea').focus();
 }
});
document.addEventListener('keydown',e=>{if(e.key==='Escape'&&settingsOpen&&!$('chat-dialog').open){closeSettings();document.querySelector('[data-action="settings"]').focus();}});
if(window.location?.hash==='#settings')openSettings();
if(window.location?.hash==='#add-device')addDeviceDialog();

if(window.location?.hash==='#settings-models')openSettings('models');

document.addEventListener('submit',e=>{
 if(e.target.id!=='agent-settings-form')return;e.preventDefault();
 const form=e.target,data=new FormData(form),node=form.dataset.device,id=form.dataset.agent;
 const name=String(data.get('name')||'').trim(),profileId=String(data.get('profile')||''),modelId=String(data.get('model')||'');
 const profile=profiles.find(profile=>profile.id===profileId&&profile.node===node);
 if(!name||!devices[node])return;
 if(profileId&&(!profile||!profile.models.some(model=>model.id===modelId))){$('agent-settings-error').hidden=false;$('agent-settings-error').textContent='请先在连接详情添加模型，再为 Agent 选择。';return;}
 if(id){if(!people[id]||people[id].node!==node)return;people[id].name=name;people[id].modelConnectionId=profileId;people[id].modelId=profileId?modelId:'';const direct=conversations.find(c=>c.kind==='leader'&&c.leaderId===id);if(direct)direct.name=name;}
 else{const role=String(data.get('role')||'Leader');if(!['Leader','Worker'].includes(role))return;const newId='agent-'+(++sequence);people[newId]={name,node,role,avatar:role==='Leader'?'fox':'cat',modelConnectionId:profileId,modelId:profileId?modelId:''};}
 $('chat-dialog').close();renderList();openSettings('agents',node);
});

if(window.location?.hash==='#add-profile'){openSettings('models');addProfileDialog();}
