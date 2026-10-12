import React, {memo,useEffect} from 'react';
import {blocksCSS,formCSS} from './fixture-styles';
const fields=[['display-name','Display name','Ada Lovelace'],['email','Email address','ada@example.test'],['organization','Organization','Analytical Engines'],['role','Role','Engineer'],['city','City','London'],['timezone','Time zone','Europe/London'],['website','Website','https://example.test'],['notes','Profile notes','Notes']];
const values=['Ada Lovelace 0','ada@example.test','Analytical Engines','Engineer','London','Europe/London','https://example.test','First line of profile notes.\nA second line exercises multiline content.'];
const Cell=memo(({color})=><div className={color?'cell filled '+color:'cell'}/>);
const Column=({children,...props})=><div {...props} style={{display:'flex',flexDirection:'column'}}>{children}</div>;
const Row=({children,...props})=><div {...props} style={{display:'flex'}}>{children}</div>;
const Text=({children,...props})=><div {...props}>{children}</div>;
const noop=()=>{};
export default function Fixture({name}) {
 useEffect(()=>{window.__fixtureReady=true;},[]);
 const count=Number(name.replace('cells-',''));
 if(name!=='empty'&&name!=='rich-form'&&![50,200,800].includes(count))throw Error('Unknown fixture');
 const style=name==='rich-form'?formCSS:name==='empty'?'':blocksCSS;
 return <><style>{style+'\nhtml,body,#root{margin:0;width:100%;height:100%;} .fixture-window{width:800px;height:720px;background:#101820;} .fixture-window *{box-sizing:border-box;}'} </style><div className="fixture-window">
 {name==='empty'?null:name==='rich-form'?<Column className="shell">
 <Text className="heading">Account details</Text><Text className="description">A representative accessible form with retained controls and scrolling.</Text>
 <div id="form-scroll" className="form-scroll" style={{overflow:'auto',height:500}}><Column className="form" role="group" aria-label="Account details form">
 {fields.map(([id,label,placeholder],i)=><Column key={id} className="field-row"><Text className="label">{label}</Text>{i===7?<textarea id={id} className="field notes" value={values[i]} placeholder={placeholder} aria-label={label} onChange={noop} onFocus={noop} onBlur={noop}/>:<input id={id} className="field" value={values[i]} placeholder={placeholder} aria-label={label} onChange={noop} onFocus={noop} onBlur={noop}/>}</Column>)}
 <label><input id="updates" type="checkbox" checked={true} aria-label="Receive product updates" onChange={noop}/>Receive product updates</label>
 <input id="volume" type="range" value={40} min={0} max={100} step={1} aria-label="Notification volume" onChange={noop}/>
 <select id="region" value="Americas" aria-label="Region" onChange={noop}><option id="americas">Americas</option><option id="europe">Europe</option><option id="asia">Asia Pacific</option></select>
 <Row className="actions"><button id="cancel" className="button" aria-label="Discard account changes" onClick={noop} onFocus={noop} onBlur={noop} onContextMenu={noop}>Cancel</button><button id="save" className="button primary" aria-label="Save account changes" onClick={noop} onFocus={noop} onBlur={noop} onContextMenu={noop}>Save changes</button></Row>
 </Column></div><Text className="status">Revision 0: Saved</Text></Column>:<Column className="app">
 <Row className="topbar"><Text className="brand">Blocks admission</Text><Text>Score 0  ·  Lines 0  ·  Level 1</Text></Row>
 <Row className="play"><div className="board" role="group" aria-label="Game board">{Array.from({length:count},(_,i)=><Cell key={i} color={i%50===0?'c2':''}/>)}</div><Column className="side"><Text className="status">In play</Text><Text>Next</Text><div className="next-piece"><div className="next-block"/></div><Text>Controls</Text><Text className="hint">Event-driven controls</Text></Column></Row>
 </Column>}
 </div></>;
}
