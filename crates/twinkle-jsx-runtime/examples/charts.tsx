const chartData: TwinkleChartData = {
  revision: 1,
  datasets: [{
    id: "sales",
    keys: ["north-jan", "south-jan", "north-feb", "south-feb"],
    columns: [
      { name: "month", type: "number", values: [1, 1, 2, 2] },
      { name: "region", type: "category", values: ["North", "South", "North", "South"] },
      { name: "revenue", type: "number", unit: "USD", values: [10, 20, 15, 25] },
      { name: "orders", type: "number", values: [3, 8, 5, 12] },
    ],
  }],
};

function ChartGallery() {
  const [visibleSeries, setVisibleSeries] = useState<ReadonlyArray<string>>(["North", "South"]);
  const toggle = ({series, visible}: Readonly<{series:string;visible:boolean}>) => {
    setVisibleSeries(previous => visible
      ? [...previous, series]
      : previous.filter(value => value !== series));
  };
  const table = (visible: boolean) => twinkle.request({type: "chart-table", visible});
  const common = {
    data: chartData,
    width: 480,
    height: 260,
    state: {visibleSeries},
    onTableMode: table,
    onNavigate: (event: Readonly<{kind:"navigate";command:string}>) => twinkle.request({type:"chart-event", ...event}),
    onSelectDatum: (event: Readonly<{kind:"select";dataset:string;row:string;series?:string;revision:number}>) => twinkle.request({type:"chart-event", ...event}),
    onInspectDatum: (event: Readonly<{kind:"inspect";dataset:string;row:string;series?:string;revision:number;cells:ReadonlyArray<TwinkleChartInspectionCell>}>) => twinkle.request({
      type:"chart-event", kind:event.kind, dataset:event.dataset, row:event.row,
      series:event.series ?? null, revision:event.revision,
      cells:event.cells.map(cell => ({column:cell.column, unit:cell.unit ?? null, formatted:cell.formatted, value:cell.value})),
    }),
    onViewportChange: (event: Readonly<{kind:"viewport";x?:readonly [number,number];y?:readonly [number,number]}>) => twinkle.request({type:"chart-event", ...event}),
    onViewportCommand: (event: Readonly<{kind:"viewportCommand";command:string}>) => twinkle.request({type:"chart-event", ...event}),
    onPan: (event: Readonly<TwinkleChartPanGesture>) => twinkle.request({type:"chart-event", ...event}),
    onZoom: (event: Readonly<TwinkleChartZoomGesture>) => twinkle.request({type:"chart-event", ...event}),
  };
  return <Column>
    <Chart {...common} id="trend" spec={{family:"line",title:"Revenue trend",dataset:"sales",x:"month",y:"revenue",series:"region"}} onToggleSeries={toggle} />
    <Chart {...common} id="area" spec={{family:"stacked-area",title:"Revenue area",dataset:"sales",x:"month",series:"region",value:"revenue"}} onToggleSeries={toggle} />
    <Chart {...common} id="bars" spec={{family:"grouped-bar",title:"Revenue by region",dataset:"sales",category:"month",series:"region",value:"revenue"}} onToggleSeries={toggle} />
    <Chart {...common} id="bubble" spec={{family:"bubble",title:"Orders and revenue",dataset:"sales",x:"orders",y:"revenue",size:"orders"}} />
    <Chart {...common} id="share" spec={{family:"donut",title:"Revenue share",dataset:"sales",category:"region",value:"revenue",innerRadius:0.55}} />
    <Chart {...common} id="distribution" spec={{family:"histogram",title:"Order distribution",dataset:"sales",value:"orders",start:0,end:16,bins:4}} />
  </Column>;
}
