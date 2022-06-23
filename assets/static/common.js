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
    if (start.getFullYear() != end.getFullYear()) {
        dateRange.textContent = start.toLocaleString([], {dateStyle: 'long'}) + '–' + end.toLocaleString([], {dateStyle: 'long'});
    } else if (start.getDate() != end.getDate()) {
        dateRange.textContent = start.toLocaleString([], {month: 'long', day: 'numeric'}) + '–' + end.toLocaleString([], {dateStyle: 'long'});
    } else {
        dateRange.textContent = start.toLocaleString([], {dateStyle: 'long'});
    }
});
