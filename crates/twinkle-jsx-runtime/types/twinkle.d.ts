// Generic native Twinkle JSX API. Host extensions are declared separately.
declare namespace JSX {
    interface Element {}
    interface IntrinsicAttributes { key?: string | number }
    interface ElementChildrenAttribute { children: {} }
    interface IntrinsicElements {
        div: TwinkleDivProps;
    }
}

type TwinkleChild = JSX.Element | string | number | null | undefined | boolean | ReadonlyArray<TwinkleChild>;
type TwinkleComponent<P = TwinkleProps> = (props: P) => JSX.Element | null;
type TwinkleAnchor = "center" | "top-left" | "top-center" | "top-right" | "bottom-left" | "bottom-center" | "bottom-right";
type TwinkleJson = null | boolean | number | string | ReadonlyArray<TwinkleJson> | { readonly [key: string]: TwinkleJson };
type TwinkleColor = number; // 0xAARRGGBB
type TwinkleClick = () => void;
interface TwinkleDragGesture {
    phase: "start" | "move" | "end" | "cancel";
    x: number;
    y: number;
    bounds: { x: number; y: number; width: number; height: number };
}
interface TwinkleDropGesture {
    x: number;
    y: number;
    sourceId: string;
    sourceBounds: { x: number; y: number; width: number; height: number };
    targetId: string;
    targetBounds: { x: number; y: number; width: number; height: number };
}

interface TwinkleProps {
    key?: string | number;
    className?: string;
    children?: TwinkleChild;
}

interface TwinkleDivProps extends TwinkleProps {
    id?: string;
    onClick?: TwinkleClick;
    onDrop?: (gesture: TwinkleDropGesture) => void;
    role?: "button" | "radio" | "radiogroup" | "option" | "group";
    "aria-label"?: string;
    "aria-checked"?: boolean;
    "aria-selected"?: boolean;
    disabled?: boolean;
}

interface TwinklePanelProps extends TwinkleProps {
    background?: TwinkleColor;
    height?: number;
}
interface TwinkleWindowProps extends TwinkleProps {
    /** Native window activation, distinct from control focus; duplicate reports are ignored. */
    onFocus?: () => void;
    /** Native focus loss after activation; initial unactivated focus loss is ignored. */
    onBlur?: () => void;
    onSubmit?: () => void;
    onEscape?: () => void;
    id?: string;
    title?: string;
    width: number | "100%";
    height: number | "100%";
    placement?: "managed" | "fixed";
    output?: "primary" | "all";
    edge?: "top" | "bottom" | "left" | "right";
    anchor?: "center" | "top-left" | "top-center" | "top-right" | "bottom-left" | "bottom-center" | "bottom-right";
    reserveWorkArea?: boolean;
    bottomOffset?: number;
    background?: TwinkleColor;
}
interface TwinkleBoxProps extends TwinkleProps {
    x: number;
    y: number;
    width: number;
    height: number;
    background?: TwinkleColor;
    radius?: number;
}
interface TwinkleTextProps extends TwinkleProps { color?: TwinkleColor; wrap?: boolean; maxLines?: number }
interface TwinkleButtonProps extends TwinkleProps {
    id?: string;
    width?: number;
    height?: number;
    accessibilityLabel?: string;
    icon?: string;
    showLabel?: boolean;
    disabled?: boolean;
    state?: "selected" | "unselected" | "disabled";
    onClick: TwinkleClick;
    onFocus?: TwinkleClick;
    onBlur?: TwinkleClick;
    onContextMenu?: TwinkleClick;
    onDrag?: (gesture: TwinkleDragGesture) => void;
    onDrop?: (gesture: TwinkleDropGesture) => void;
}
interface TwinkleTextFieldProps extends TwinkleProps {
    id?: string;
    value?: string;
    accessibilityLabel?: string;
    disabled?: boolean;
    placeholder?: string;
    /** Mask the displayed value and protect the surface from remote inspection. */
    secure?: boolean;
    onChange: (value: string) => void;
    onFocus?: TwinkleClick;
    onBlur?: TwinkleClick;
}
interface TwinkleSliderProps extends TwinkleProps {
    id?: string;
    value: number;
    accessibilityLabel: string;
    /** Defaults to 0..1; onChange receives a value in this numeric range. */
    min?: number;
    max?: number;
    step?: number;
    onChange: (value: number) => void;
}
interface TwinkleSwitchProps extends TwinkleProps {
    id: string;
    state: "on" | "off" | "disabled-on" | "disabled-off";
    accessibilityLabel: string;
    onClick?: TwinkleClick;
}
interface TwinkleColorSwatchProps extends TwinkleProps {
    id?: string;
    /** CSS color; omit for the custom-color button. */
    color?: string;
    selected?: boolean;
    accessibilityLabel: string;
    onClick: TwinkleClick;
}
interface TwinkleSelectProps extends TwinkleProps {
    id: string;
    value: string;
    open?: boolean;
    accessibilityLabel: string;
    onClick: TwinkleClick;
}
interface TwinkleOptionProps extends TwinkleProps {
    /** Optional package or application artwork asset key. */
    icon?: string;
    id: string;
    onClick: TwinkleClick;
}
interface TwinkleImageProps extends TwinkleProps {
    asset: string;
    width: number;
    height: number;
    fit?: "contain" | "cover" | "stretch";
}
interface TwinkleImageButtonProps extends TwinkleImageProps {
    id?: string;
    accessibilityLabel: string;
    onClick: TwinkleClick;
    onContextMenu?: TwinkleClick;
}
interface TwinkleDialogProps extends TwinkleProps {
    id?: string;
    anchor: string;
    open?: boolean;
    onClose?: TwinkleClick;
    width?: number;
    height?: number;
}
interface TwinkleMenuProps extends TwinkleProps {
    id: string;
    anchor: string;
    open?: boolean;
    x?: number;
    y?: number;
}
interface TwinkleMenuItemProps extends TwinkleProps {
    id: string;
    label?: string;
    onClick?: TwinkleClick;
    disabledReason?: string;
    shortcut?: string;
    separatorBefore?: boolean;
}
interface TwinkleBadgeProps extends TwinkleProps {
    item?: string;
    label?: string;
    count: number;
    color?: TwinkleColor;
}
interface TwinkleProgressProps extends TwinkleProps {
    percent: number;
    width: number;
    height: number;
}
type TwinkleChartValue = number | string | boolean | null;
interface TwinkleChartColumn {
    readonly name: string;
    readonly type: "number" | "timestamp" | "category" | "boolean";
    readonly unit?: string;
    readonly values: ReadonlyArray<TwinkleChartValue>;
}
interface TwinkleChartDataset {
    readonly id: string;
    readonly keys: ReadonlyArray<string>;
    readonly columns: ReadonlyArray<TwinkleChartColumn>;
}
interface TwinkleChartData {
    /** Safe nonnegative integer identifying this immutable snapshot. */
    readonly revision: number;
    readonly datasets: ReadonlyArray<TwinkleChartDataset>;
}
type TwinkleChartFamily = "line" | "area" | "stacked-area" | "scatter" | "bubble" |
    "bar" | "horizontal-bar" | "grouped-bar" | "grouped-horizontal-bar" |
    "stacked-bar" | "stacked-horizontal-bar" | "pie" | "donut" | "histogram" |
    "heatmap" | "sparkline";
interface TwinkleChartRecipeSpec {
    readonly family: TwinkleChartFamily;
    readonly title: string;
    readonly summary?: string;
    readonly dataset: string;
    readonly x?: string; readonly y?: string; readonly series?: string;
    readonly category?: string; readonly value?: string; readonly size?: string;
    readonly color?: string; readonly normalized?: boolean;
    readonly innerRadius?: number;
    readonly start?: number; readonly end?: number; readonly bins?: number; readonly density?: boolean;
}
type TwinkleChartChannel = "x" | "x2" | "y" | "y2" | "series" | "group" | "fill" | "stroke" | "opacity" | "symbol" | "size" | "text";
type TwinkleChartMark = "rect" | "rule" | "text" | Readonly<
    {kind:"bar";orientation?:"vertical"|"horizontal"} |
    {kind:"line";interpolation?:"straight"|"step-before"|"step-after";points?:boolean} |
    {kind:"area";interpolation?:"straight"|"step-before"|"step-after";baseline?:number} |
    {kind:"point";symbol?:"circle"|"square"|"triangle"|"diamond"|"cross"} |
    {kind:"arc";innerRadius?:number}
>;
type TwinkleChartTransform = Readonly<
    {kind:"sort";field:string;descending?:boolean} |
    {kind:"bin";field:string;output:string;start:number;end:number;count:number;density?:boolean} |
    {kind:"aggregate";groupBy:ReadonlyArray<string>;field?:string;output:string;operation:"count"|"sum"|"mean"|"min"|"max";missing?:"omit"|"propagate"} |
    {kind:"stack";category:string;series:string;value:string;mode?:"raw"|"normalized"}
>;
interface TwinkleChartGrammarSpec {
    readonly family: "custom";
    readonly title: string;
    readonly summary?: string;
    readonly dataset: string;
    readonly visibilityDomain?: "original" | "visible-series";
    readonly scales?: ReadonlyArray<Readonly<{name:string;kind:"linear"|"log"|"time"|"ordinal"|"band"|"color"|"size";domain?:readonly [number,number];clamp?:boolean;reversed?:boolean;unit?:string;base?:number;innerPadding?:number;outerPadding?:number}>>;
    readonly layers: ReadonlyArray<Readonly<{mark:TwinkleChartMark;encodings:ReadonlyArray<Readonly<{channel:TwinkleChartChannel;field:string;dataset?:string;scale?:string}>>;transforms?:ReadonlyArray<TwinkleChartTransform>;visible?:boolean}>>;
    readonly axes?: ReadonlyArray<Readonly<{scale:string;title?:string;unit?:string;tickCount?:number;grid?:boolean}>>;
    readonly legends?: ReadonlyArray<Readonly<{scale:string;title?:string;toggleable?:boolean}>>;
    readonly annotations?: ReadonlyArray<Readonly<
        {kind:"rule";scale:string;value:number;label?:string;includeInDomain?:boolean} |
        {kind:"band";scale:string;start:number;end:number;label?:string;includeInDomain?:boolean} |
        {kind:"text";text:string;x:number;y:number}
    >>;
    readonly facet?: Readonly<{row?:string|Readonly<{dataset?:string;field:string}>;column?:string|Readonly<{dataset?:string;field:string}>;scales?:"shared"|"independent"}>;
}
type TwinkleChartSpec = TwinkleChartRecipeSpec | TwinkleChartGrammarSpec;
interface TwinkleChartState {
    readonly selectedRows?: ReadonlyArray<string>;
    readonly visibleSeries?: ReadonlyArray<string>;
    readonly viewport?: { readonly x?: readonly [number, number]; readonly y?: readonly [number, number] };
    /** Exact accepted datum rendered as a passive, plot-bounded inspector. */
    readonly inspection?: { readonly dataset: string; readonly row: string };
}
interface TwinkleChartInspectionCell {
    readonly column: string;
    readonly unit?: string;
    readonly formatted: string;
    readonly value: number | string | boolean | null;
}
interface TwinkleChartPanGesture extends TwinkleDragGesture { readonly kind: "pan" }
interface TwinkleChartZoomGesture {
    readonly kind: "zoom";
    readonly x: number;
    readonly y: number;
    readonly deltaX: number;
    readonly deltaY: number;
}
interface TwinkleChartProps extends TwinkleProps {
    id: string;
    width: number;
    height: number;
    data: TwinkleChartData;
    spec: TwinkleChartSpec;
    state?: TwinkleChartState;
    onTableMode: (visible: boolean) => void;
    onToggleSeries?: (event: Readonly<{series:string;visible:boolean}>) => void;
    onNavigate?: (event: Readonly<{kind:"navigate";command:string}>) => void;
    onSelectDatum?: (event: Readonly<{kind:"select";dataset:string;row:string;series?:string;revision:number}>) => void;
    onInspectDatum?: (event: Readonly<{kind:"inspect";dataset:string;row:string;series?:string;revision:number;cells:ReadonlyArray<TwinkleChartInspectionCell>}>) => void;
    onViewportChange?: (event: Readonly<{kind:"viewport";x?:readonly [number,number];y?:readonly [number,number]}>) => void;
    onViewportCommand?: (event: Readonly<{kind:"viewportCommand";command:string}>) => void;
    /** Opts into captured native drag delivery; update the controlled viewport in application state. */
    onPan?: (event: Readonly<TwinkleChartPanGesture>) => void;
    /** Opts into wheel/trackpad delivery; omit this callback to preserve enclosing scroll ownership. */
    onZoom?: (event: Readonly<TwinkleChartZoomGesture>) => void;
}
declare function Fragment(props:TwinkleProps):JSX.Element;
declare function h(kind: unknown, props?: object | null, ...children: TwinkleChild[]): JSX.Element;
/** @deprecated Compatibility helper; use Window or FixedWindow for new surfaces. */
declare function Panel(props: TwinklePanelProps): JSX.Element;
declare function Window(props: TwinkleWindowProps): JSX.Element;
/** Convenience JSX component that renders Window with fixed placement. */
declare function FixedWindow(props: Omit<TwinkleWindowProps, "placement">): JSX.Element;
declare function Box(props: TwinkleBoxProps): JSX.Element;
/** Positions Box children by their x and y coordinates. Size it with CSS. */
declare function Layer(props: TwinkleProps & { id?: string }): JSX.Element;
/** Generic CSS layout box. Defaults to block layout. */
declare function Div(props: TwinkleDivProps): JSX.Element;
declare function Badge(props: TwinkleBadgeProps): JSX.Element;
declare function Row(props: TwinkleProps): JSX.Element;
declare function Column(props: TwinkleProps): JSX.Element;
declare function ScrollView(props: TwinkleProps & { id: string; height?: number; grow?: boolean }): JSX.Element;
/** Embedded virtual rows selected by the native ancestor viewport. Stable item keys must be unique, nonempty, and at most 512 UTF-8 bytes. Native row slots have a minimum height of one logical pixel, matching Collection; itemHeight supplies initial estimates. */
declare function VirtualColumn<T>(props: { id: string; className?: string; items: readonly T[]; itemKey: (item: T, index: number) => string; itemHeight: number | ((item: T, index: number) => number); gap?: number; overscan?: number; renderItem: (item: T, index: number) => JSX.Element }): JSX.Element;
declare function Text(props: TwinkleTextProps): JSX.Element;
declare function Image(props: TwinkleImageProps): JSX.Element;
declare function ImageButton(props: TwinkleImageButtonProps): JSX.Element;
declare function Progress(props: TwinkleProgressProps): JSX.Element;
declare function TextField(props: TwinkleTextFieldProps): JSX.Element;
declare function Checkbox(props: TwinkleProps & { id?: string; checked?: boolean; indeterminate?: boolean; disabled?: boolean; label?: string; accessibilityLabel?: string; onChange?: (checked: boolean) => void; onClick?: TwinkleClick }): JSX.Element;
declare function Slider(props: TwinkleSliderProps): JSX.Element;
declare function Switch(props: TwinkleSwitchProps): JSX.Element;
declare function ColorSwatch(props: TwinkleColorSwatchProps): JSX.Element;
declare function Select(props: TwinkleSelectProps): JSX.Element;
declare function Option(props: TwinkleOptionProps): JSX.Element;
declare function Button(props: TwinkleButtonProps): JSX.Element;
/** Feature-gated native chart. Hosts without the charts capability reject admission. */
declare function Chart(props: TwinkleChartProps): JSX.Element;
declare function Spacer(props: TwinkleProps): JSX.Element;
/** Host-provided content placed inside this component's layout box. */
declare function Slot(props: TwinkleProps & { id: string }): JSX.Element;
declare function Dialog(props: TwinkleDialogProps): JSX.Element;
declare function Menu(props: TwinkleMenuProps): JSX.Element;
declare function MenuItem(props: TwinkleMenuItemProps): JSX.Element;

declare function useState<T>(initial: T | (() => T)): [T, (next: T | ((previous: T) => T)) => void];
declare function useReducer<S, A>(reducer: (state: S, action: A) => S, initialState: S): [S, (action: A) => void];
declare function useReducer<S, A, I>(reducer: (state: S, action: A) => S, initialArg: I, init: (initialArg: I) => S): [S, (action: A) => void];
declare function useRef<T>(initial: T): { current: T };
/** Stable, opaque ID for accessibility relationships within this component's mounted surface. */
declare function useId(): string;
interface TwinkleContext<T> { readonly Provider: TwinkleComponent<{value:T;children?:TwinkleChild}>; readonly defaultValue: T }
declare function createContext<T>(defaultValue: T): TwinkleContext<T>;
declare function useContext<T>(context: TwinkleContext<T>): T;

interface TwinkleLocaleSnapshot {readonly generation:number;readonly tag:string;readonly direction:"ltr"|"rtl";readonly known:boolean}
declare function useLocale():Readonly<TwinkleLocaleSnapshot>;
interface TwinkleExternalStore<T> {readonly subscribe:(listener:()=>void)=>()=>void;readonly getSnapshot:()=>T}

/** Only matching functions from a host-branded store entry are accepted. */
declare function useSyncExternalStore<T>(subscribe:TwinkleExternalStore<T>["subscribe"],getSnapshot:TwinkleExternalStore<T>["getSnapshot"]):T;
declare function memo<P>(component:TwinkleComponent<P>,compare?:(previous:Readonly<P & {children?:TwinkleChild}>,next:Readonly<P & {children?:TwinkleChild}>)=>boolean):TwinkleComponent<P>;
interface TwinkleComponentFailure { readonly message:string }
interface TwinkleErrorBoundaryProps {
    readonly children?:TwinkleChild;
    readonly fallback?:TwinkleChild | ((error:Readonly<TwinkleComponentFailure>,reset:()=>void)=>TwinkleChild);
    /** A changed member resets a failed boundary before its next render. */
    readonly resetKeys?:ReadonlyArray<unknown>;
}
declare const ErrorBoundary:TwinkleComponent<TwinkleErrorBoundaryProps>;
interface TwinkleThemePalette {
    readonly background:TwinkleColor; readonly panel:TwinkleColor; readonly surface:TwinkleColor;
    readonly surfaceHover:TwinkleColor; readonly text:TwinkleColor; readonly muted:TwinkleColor;
    readonly accent:TwinkleColor; readonly accentSoft:TwinkleColor; readonly complement:TwinkleColor;
}
interface TwinkleThemeSnapshot {
    readonly generation:number;
    readonly mode:"light"|"dark"|"unknown";
    readonly accent:TwinkleColor|null;
    readonly accentHue:number|null;
    readonly accentIntensity:number|null;
    /** null means the host has not bridged an authoritative preference. */
    readonly reducedMotion:boolean|null;
    /** null means the host has not bridged an authoritative preference. */
    readonly reducedTransparency:boolean|null;
    readonly palette:Readonly<TwinkleThemePalette>|null;
}
declare function useTheme(): Readonly<TwinkleThemeSnapshot>;
declare function useTheme<T>(selector: (theme: Readonly<TwinkleThemeSnapshot>) => T): T;
declare function useReducedMotion(): boolean|null;

declare function useEffect(setup: () => void | (() => void), dependencies?: ReadonlyArray<unknown>): void;
interface TwinkleSurfaceSnapshot {
    readonly generation:number;
    /** Opaque host-owned identity for this exact mounted surface. */
    readonly mountId:string|null;
    readonly id:string|null;
    readonly kind:"panel"|"dock"|"desktop"|"window"|"dialog"|"overlay"|null;
    readonly logicalSize:Readonly<{width:number;height:number}>|null;
    /** Unavailable until the native output authority is projected to this mount. */
    readonly output:string|null;
    readonly availableSize:Readonly<{width:number;height:number}>|null;
    readonly scaleFactor:number|null;
    readonly focused:boolean|null;
    readonly visible:boolean|null;
}
declare function useSurface():TwinkleSurfaceSnapshot;
declare function useSurface<T>(selector:(surface:TwinkleSurfaceSnapshot)=>T):T;
declare function useOutput():TwinkleOutputSnapshot|null;
declare function useScaleFactor():number|null;
declare function useSurfaceFocus():boolean|null;
declare function useMemo<T>(factory: () => T, dependencies?: ReadonlyArray<unknown>): T;
declare function useCallback<T extends (...args: never[]) => unknown>(callback: T, dependencies?: ReadonlyArray<unknown>): T;


interface TwinkleOutputSnapshot { readonly name:string }
interface TwinkleHostCapabilitySnapshot {
 readonly declared:boolean;
 readonly available:boolean|null;
 readonly reason:string|null;
}
declare function useHostCapability(name:string):Readonly<TwinkleHostCapabilitySnapshot>;
declare const TwinkleStores:Readonly<{
 locale:TwinkleExternalStore<Readonly<TwinkleLocaleSnapshot>>;
 theme:TwinkleExternalStore<Readonly<TwinkleThemeSnapshot>>;
 capabilities:TwinkleExternalStore<Readonly<Record<string,Readonly<TwinkleHostCapabilitySnapshot>>>>;
}>;
interface TwinkleContribution<P = TwinkleProps> {
 readonly key:string;
 readonly component:TwinkleComponent<P>;
 readonly [metadata:string]:unknown;
}
declare const twinkle:Readonly<{
 readonly data:Readonly<Record<string,unknown>>;
 request(effect:TwinkleJson):void;
 component<P=TwinkleProps>(contract:string):TwinkleComponent<P>;
 contributions<P=TwinkleProps>(collection:string):ReadonlyArray<Readonly<TwinkleContribution<P>>>;
}>;
