document.querySelectorAll('.datetime').forEach(function(dateTime) {
    var longFormat = dateTime.dataset.long == 'true';
    dateTime.textContent = new Date(parseInt(dateTime.dataset.timestamp)).toLocaleString([], {
        dateStyle: longFormat ? 'full' : 'medium',
        timeStyle: longFormat ? 'full' : 'short',
    });
});

document.querySelectorAll('.daterange').forEach(function(dateRange) {
    var start = new Date(parseInt(dateRange.dataset.start));
    var end = new Date(parseInt(dateRange.dataset.end));
    dateRange.textContent = Intl.DateTimeFormat([], {dateStyle: 'long'}).formatRange(start, end);
});
