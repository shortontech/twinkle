import React, { useCallback, useEffect, useRef, useState } from 'react';

import { Game } from './model.js';

function Diagnostics({ ticks, renders, active }) {
  return <div className="diagnostics" aria-label="Renderer diagnostics"><span>Ticks <output>{ticks}</output></span><span>App renders <output>{renders}</output></span><span>Loop <output>{active ? 'running' : 'idle'}</output></span></div>;
}
function Cell({ color, index }) {
  return <div className={color ? `cell filled ${color}` : 'cell'} data-cell={index} />;
}

export default function App({ controlled = false }) {
  const [game, setGame] = useState(() => new Game());
  const [menuOpen, setMenuOpen] = useState(false);
  const [focused, setFocused] = useState(() => document.hasFocus());
  const [visible, setVisible] = useState(() => document.visibilityState === 'visible');
  const [ticks, setTicks] = useState(0);
  const renderCounter = useRef(0); renderCounter.current++;
  const gameRef = useRef(game), menuRef = useRef(menuOpen);
  gameRef.current = game; menuRef.current = menuOpen;
  const input = useRef({ axisX: 0, horizontalDirection: 0, gravityElapsed: 0, horizontalElapsed: 0, down: false, previousButtons: [] });
  const active = !controlled && !menuOpen && !game.paused && !game.over && focused && visible;
  const publish = useCallback(next => { gameRef.current = next; setGame(next); }, []);
  const dispatch = useCallback(action => {
    const next = new Game(gameRef.current); next.act(action); publish(next);
  }, [publish]);
  const toggleMenu = useCallback(() => {
    if (gameRef.current.over) return;
    setMenuOpen(value => !value); menuRef.current = !menuRef.current;
    input.current.axisX = 0; input.current.horizontalDirection = 0; input.current.horizontalElapsed = 0;
  }, []);
  useEffect(() => {
    const clearInput = () => {
      input.current.down = false; input.current.axisX = 0;
      input.current.horizontalDirection = 0; input.current.horizontalElapsed = 0;
      input.current.previousButtons = [];
    };
    const focus = () => setFocused(true), blur = () => { clearInput(); setFocused(false); };
    const visibility = () => {
      const shown = document.visibilityState === 'visible';
      if (!shown) clearInput();
      setVisible(shown);
    };
    window.addEventListener('focus', focus); window.addEventListener('blur', blur); document.addEventListener('visibilitychange', visibility);
    return () => { window.removeEventListener('focus', focus); window.removeEventListener('blur', blur); document.removeEventListener('visibilitychange', visibility); };
  }, []);
  // Browser replacement for Twinkle's active-only poll_controller interval.
  useEffect(() => {
    if (!active) return undefined;
    let frame = 0, last = performance.now();
    const poll = now => {
      const interval = Math.min(100, now - last); last = now;
      let next = new Game(gameRef.current);
      const pads = navigator.getGamepads ? [...navigator.getGamepads()] : [];
      const pad = pads.find(item => item?.connected);
      const buttons = pad?.buttons ?? [], axes = pad?.axes ?? [];
      const edge = index => Boolean(buttons[index]?.pressed && !input.current.previousButtons[index]);
      if (edge(9)) { toggleMenu(); input.current.previousButtons = buttons.map(item => Boolean(item?.pressed)); frame = requestAnimationFrame(poll); return; }
      if (edge(0)) next.act('rotate');
      if (edge(1)) next.act('drop');
      if (edge(2)) next.act('down');
      if (buttons[12]?.pressed && !input.current.previousButtons[12]) next.act('rotate');
      const down = input.current.down || buttons[13]?.pressed || (axes[1] ?? 0) > 0.65;
      if (down) {
        input.current.gravityElapsed += interval;
        if (input.current.gravityElapsed >= next.softDropInterval()) {
          input.current.gravityElapsed -= next.softDropInterval(); next.act('down');
        }
      } else {
        input.current.gravityElapsed += interval;
        if (input.current.gravityElapsed >= next.gravityInterval()) {
          input.current.gravityElapsed -= next.gravityInterval(); next.gravityTick(); setTicks(count => count + 1);
        }
      }
      const axis = axes[0] ?? 0;
      const direction = buttons[14]?.pressed || axis < -0.65 ? -1 : buttons[15]?.pressed || axis > 0.65 ? 1 : 0;
      if (direction === 0 || direction !== input.current.horizontalDirection) {
        input.current.horizontalDirection = direction; input.current.horizontalElapsed = 0;
        if (direction !== 0) next.act(direction < 0 ? 'left' : 'right');
      } else {
        input.current.horizontalElapsed += interval;
        if (input.current.horizontalElapsed >= 150) {
          next.act(direction < 0 ? 'left' : 'right'); input.current.horizontalElapsed = 100;
        }
      }
      input.current.axisX = axis; input.current.previousButtons = buttons.map(item => Boolean(item?.pressed));
      if (JSON.stringify(next) !== JSON.stringify(gameRef.current)) publish(next);
      frame = requestAnimationFrame(poll);
    };
    frame = requestAnimationFrame(poll);
    return () => cancelAnimationFrame(frame);
  }, [active, game, publish, toggleMenu]);
  useEffect(() => {
    const keyDown = event => {
      if (event.repeat) return;
      if (event.key === 'Escape') { event.preventDefault(); toggleMenu(); return; }
      if (menuRef.current) return;
      const action = event.key === 'ArrowUp' ? 'rotate' : event.key === 'Enter' ? 'drop' : null;
      if (event.key === 'ArrowDown') { event.preventDefault(); input.current.down = true; dispatch('down'); }
      else if (event.altKey && event.key === 'ArrowLeft') { event.preventDefault(); dispatch('left'); }
      else if (event.altKey && event.key === 'ArrowRight') { event.preventDefault(); dispatch('right'); }
      else if (action) { event.preventDefault(); dispatch(action); }
    };
    const keyUp = event => { if (event.key === 'ArrowDown') input.current.down = false; };
    window.addEventListener('keydown', keyDown); window.addEventListener('keyup', keyUp);
    return () => { window.removeEventListener('keydown', keyDown); window.removeEventListener('keyup', keyUp); };
  }, [dispatch, toggleMenu]);
  // The benchmark controls only the model clock. Rendering, listeners and
  // menu actions use this frozen production component unchanged.
  useEffect(() => {
    if (!controlled) return;
    window.__blocksAPI = {
      get: () => ({game: gameRef.current, menuOpen: menuRef.current}),
      set: (next, menu) => { gameRef.current = next; menuRef.current = menu; setGame(next); setMenuOpen(menu); },
      focus: setFocused, visibility: setVisible,
    };
    return () => { delete window.__blocksAPI; };
  }, [controlled]);
  const data = game.snapshot(menuOpen);
  const act = action => dispatch(action);
  const board = data.board || [];
  const cell = (color, index) => <Cell key={index} color={color} index={index} />;
  if (data.menuOpen || data.over) {
    return <div id="main" className="menu-screen">
      <div className="menu-title">{data.over ? 'Game over' : 'Twinkle Blocks'}</div>
      <div className="menu-score">{'Score ' + data.score + '  ·  Lines ' + data.lines + '  ·  Level ' + data.level}</div>
      <div className="menu-card">
        {!data.over && <button className="menu-action primary" id="continue" onClick={() => { setMenuOpen(false); menuRef.current = false; }}>Continue</button>}
        <button className="menu-action" id="restart" onClick={() => { const restarted = new Game(); restarted.act('restart'); publish(restarted); setMenuOpen(false); menuRef.current = false; input.current.gravityElapsed = 0; }}>New game</button>
        <div className="menu-help">D-pad or left stick: move and soft drop</div>
        <div className="menu-help">A: rotate · B: hard drop · Start: resume</div>
      </div>
      <Diagnostics ticks={ticks} renders={renderCounter.current} active={active} />
    </div>;
  }
  return <div id="main" className="app">
    <div className="topbar">
      <div className="brand">Twinkle Blocks</div>
      <div>{'Score ' + data.score + '  ·  Lines ' + data.lines + '  ·  Level ' + data.level}</div>
      <div className="menu-hint">Start / Esc: game menu</div>
    </div>
    <div className="play">
      <div className="board" role="group" aria-label="Game board">{board.map(cell)}</div>
      <div className="side">
        <div className={data.over ? 'status game-over' : 'status'}>{String(data.message || '')}</div>
        <div>Next</div>
        <div className="next-piece"><div className="next-block" /></div>
        <div>Controls</div>
        <div className="hint">D-pad / left stick: move and drop · A: rotate · B: hard drop · Start: game menu. Keyboard: ↑/↓, Alt+←/→, Enter, Escape.</div>
        <Diagnostics ticks={ticks} renders={renderCounter.current} active={active} />
      </div>
    </div>
  </div>;
}
