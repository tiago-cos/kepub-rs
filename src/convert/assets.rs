//! Default contents for the two assets conversion injects.
//!
//! These are just defaults — [`crate::Converter`]'s `css_contents` and
//! `js_contents` fields override them, so shipping your own stylesheet or
//! script doesn't require touching this file.

/// Archive-root-relative path the stylesheet is written to, and the target
/// each content document's injected `<link>` resolves against.
///
/// Internal: the href isn't configurable, since it has to agree with where
/// [`crate::Converter`] actually writes the file.
pub const KOBO_CSS_HREF: &str = "css/kobo.css";

/// Archive-root-relative path the script is written to, and the target
/// each content document's injected `<script>` resolves against.
///
/// Internal: the href isn't configurable, since it has to agree with where
/// [`crate::Converter`] actually writes the file.
pub const KOBO_JS_HREF: &str = "js/kobo.js";

/// Kobo's default stylesheet.
///
/// Extracted from official Kobo-produced kepubs and adjusted to be
/// non-breaking as a standalone default.
///
/// Note the relationship to the `kobostylehacks` `<style>` block that
/// [`crate::convert::transform`] injects into every content document: the
/// last rule here is the same selector and `font-size` with a
/// `line-height` added, matching the value set on `body` above. The inline
/// block appears to be an older, smaller version of this same styling, so
/// the two overlapping is expected rather than a mistake.
pub const KOBO_CSS: &str = r"html
{
	height: 100% !important;
	margin: 0 !important;
}
body
{
	margin: 0 !important;
	height: 100% !important;
	padding-left: 0 !important;
	padding-right: 0 !important;
	padding-top: 0 !important;
}
div#book-inner p, div#book-inner div
{
    font-size: 1.0em;
}
";

/// Kobo's default reader script.
///
/// Handles pagination, progress tracking, and reader settings (font size,
/// line height, night mode) for the `book-columns`/`book-inner` layout
/// that [`crate::convert::transform`] wraps the body in. Extracted from
/// official Kobo-produced kepubs and adjusted to be non-breaking as a
/// standalone default.
pub const KOBO_JS: &str = r##"
var gPosition = 0;
var gProgress = 0;
var gCurrentPage = 0;
var gPageCount = 0;
var gClientHeight = null;

const kMaxFont = 0;

function getPosition()
{
	return gPosition;
}

function getProgress()
{
	return gProgress;
}

function getPageCount()
{
	return gPageCount;
}

function getCurrentPage()
{
	return gCurrentPage;
}

function setupBookColumns()
{
	var body = document.getElementsByTagName('body')[0].style;
	body.marginLeft = 0;
	body.marginRight = 0;
	body.marginTop = 0;
	body.marginBottom = 0;
	
  var bc = document.getElementById('book-columns').style;
  bc.width = (window.innerWidth * 2) + 'px !important';
  bc.height = (window.innerHeight-kMaxFont) + 'px !important';
  bc.marginTop = '0px !important';
  bc.webkitColumnWidth = window.innerWidth + 'px !important';
  bc.webkitColumnGap = '0px';
	bc.overflow = 'visible';

	gCurrentPage = 1;
	gProgress = gPosition = 0;
	
	var bi = document.getElementById('book-inner').style;
	bi.marginLeft = '0px';
	bi.marginRight = '0px';
	bi.padding = '0';

	gPageCount = document.body.scrollWidth / window.innerWidth;

	if (gClientHeight < (window.innerHeight-kMaxFont)) {
		gPageCount = 1;
	}
}

function paginate()
{	

	if (gClientHeight == undefined) {
		gClientHeight = document.getElementById('book-columns').clientHeight;
	}
	
	setupBookColumns();
}

function paginateAndMaintainProgress()
{
	var savedProgress = gProgress;
	setupBookColumns();
	goProgress(savedProgress);
}

function updateProgress()
{
	gProgress = (gCurrentPage - 1.0) / gPageCount;
}

function goBack()
{
	if (gCurrentPage > 1)
	{
		gCurrentPage--;
		gPosition -= window.innerWidth;
		window.scrollTo(gPosition, 0);
		updateProgress();
	}
}

function goForward()
{
	if (gCurrentPage < gPageCount)
	{
		gCurrentPage++;
		gPosition += window.innerWidth;
		window.scrollTo(gPosition, 0);
		updateProgress();
	}
}

function goPage(pageNumber)
{
	if (pageNumber > 0 && pageNumber <= gPageCount)
	{
		gCurrentPage = pageNumber;
		gPosition = (gCurrentPage - 1) * window.innerWidth;
		window.scrollTo(gPosition, 0);
		updateProgress();
	}
}

function goProgress(progress)
{
	progress += 0.0001;
	
	var progressPerPage = 1.0 / gPageCount;
	var newPage = 0;
	
	for (var page = 0; page < gPageCount; page++) {
		var low = page * progressPerPage;
		var high = low + progressPerPage;
		if (progress >= low && progress < high) {
			newPage = page;
			break;
		}
	}
		
	gCurrentPage = newPage + 1;
	gPosition = (gCurrentPage - 1) * window.innerWidth;
	window.scrollTo(gPosition, 0);
	updateProgress();		
}

function setFontFamily(newFont) {
	document.body.style.fontFamily = newFont + " !important";
	paginateAndMaintainProgress();
}

function setFontSize(toSize) {
	document.getElementById('book-inner').style.fontSize = toSize + "em !important";
	//To prevent 1 page chapters from not reflowing to additional pages when increasing the font size:
	if (toSize > 1) {
		gClientHeight = document.getElementById('book-columns').clientHeight;
	}
	paginateAndMaintainProgress();
}

function setLineHeight(toHeight) {
	document.getElementById('book-inner').style.lineHeight = toHeight + "em !important";
	paginateAndMaintainProgress();
}

function enableNightReading() {
	document.body.style.backgroundColor = "#000000";
	var theDiv = document.getElementById('book-inner');
	theDiv.style.color = "#ffffff";
	
	var anchorTags;
	anchorTags = theDiv.getElementsByTagName('a');
	
	for (var i = 0; i < anchorTags.length; i++) {
		anchorTags[i].style.color = "#ffffff";
	}
};
"##;
