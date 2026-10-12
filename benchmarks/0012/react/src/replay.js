import { Game } from './model.js';

// Explicit native application cadence is the normalized replay contract. The
// original App's animation-frame polling remains available outside controlled
// replay and is reported as a separate natural-cadence workload.
export class ReplayController {
  constructor(api) {
    this.api = api;
    this.focused = true; this.visible = true;
    this.held = {left:false,right:false,down:false};
    this.axisX = 0; this.gravityElapsedMs = 0;
    this.horizontalDirection = 0; this.horizontalElapsedMs = 0;
    this.publications = 0;
  }
  resetHorizontal() { this.axisX=0; this.horizontalDirection=0; this.horizontalElapsedMs=0; }
  state() {
    const {game, menuOpen} = this.api.get();
    return {
      presentation: game.snapshot(menuOpen),
      model: {cells:[...game.cells],shape:game.shape,x:game.x,y:game.y,rotation:game.rotation,next:game.next,
        score:game.score,lines:game.lines,paused:game.paused,over:game.over},
      bookkeeping: {axisX:this.axisX,gravityElapsedMs:this.gravityElapsedMs,
        horizontalDirection:this.horizontalDirection,horizontalElapsedMs:this.horizontalElapsedMs},
      focused:this.focused,visible:this.visible,held:{...this.held},
    };
  }
  step(step) {
    const old = this.api.get();
    const previous = JSON.stringify(old.game);
    let game = new Game(old.game), menu = old.menuOpen;
    let attempted = false, commit = false;
    const active = !menu && !game.paused && !game.over;
    switch(step.kind) {
      case 'observe': return false;
      case 'poll': {
        if (!active || !this.focused || !this.visible) return false;
        let interval = game.gravityInterval();
        if (this.held.down) interval=Math.min(interval,game.softDropInterval());
        if (this.held.left||this.held.right) interval=Math.min(interval,50);
        if (this.held.down) game.act('down');
        else {
          this.gravityElapsedMs += interval;
          if(this.gravityElapsedMs>=game.gravityInterval()) {
            this.gravityElapsedMs-=game.gravityInterval(); game.gravityTick();
          }
        }
        const direction=this.held.left?-1:this.held.right?1:0;
        if(direction===0||direction!==this.horizontalDirection) {
          this.horizontalDirection=direction; this.horizontalElapsedMs=0;
        } else {
          this.horizontalElapsedMs+=interval;
          if(this.horizontalElapsedMs>=150) {
            game.act(direction<0?'left':'right'); this.horizontalElapsedMs=100;
          }
        }
        // Same full model comparison paid by the frozen browser polling path.
        commit=JSON.stringify(game)!==previous;
        attempted=commit;
        break;
      }
      case 'button': {
        if (step.button in this.held) this.held[step.button]=step.pressed;
        if (step.pressed&&active) {
          const action={left:'left',right:'right',down:'down',rotate:'rotate',drop:'drop'}[step.button];
          if(!action)throw Error(`Unknown button ${step.button}`);
          game.act(action);
          // Browser gamepad polling does not publish unchanged model values.
          commit=JSON.stringify(game)!==previous; attempted=commit;
        }
        break;
      }
      case 'menu': {
        if(game.over)return false;
        this.api.menu(); this.resetHorizontal(); this.publications++;
        return true;
      }
      case 'click': {
        this.api.click(step.target); this.resetHorizontal();
        if(step.target==='restart')this.gravityElapsedMs=0;
        this.publications++; return true;
      }
      case 'focus':
      case 'visibility': {
        if(step.kind==='focus') {this.focused=step.value; this.api.focus(step.value);}
        else {this.visible=step.value; this.api.visibility(step.value);}
        if(!step.value) {this.held={left:false,right:false,down:false};this.axisX=0;}
        return false;
      }
      default: throw Error(`Unknown trace step ${step.kind}`);
    }
    if(commit) {this.api.set(game,menu);this.publications++;}
    return attempted;
  }
}
