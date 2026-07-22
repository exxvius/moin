export function DownloadsView() {
  return (
    <div className="view">
      <div className="view-head">
        <h2>Downloads</h2>
        <p>Live progress for everything currently downloading.</p>
      </div>

      <div className="card">
        <div className="card-title">Nothing downloading yet</div>
        <p className="dim">
          Active tasks will show up here once the engines land.
        </p>
      </div>
    </div>
  );
}
