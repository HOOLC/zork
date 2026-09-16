"""Physical input navigation shared by the liquid playground browser checks.

Opening the directory, parameter panel or state selector is part of the UI
workflow. Helpers never mutate the Rust fixture to reveal hidden controls.
"""
import re
import time


class PlaygroundInputs:
    def __init__(self, page, snapshot):
        self.page = page
        self.snapshot = snapshot

    def element(self, control_id):
        return next((e for e in self.snapshot()['elements'] if e['id'] == control_id), None)

    def frame(self):
        self.page.evaluate('()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r)))')

    @staticmethod
    def library(control_id):
        return control_id.startswith(('liquid-tab-', 'liquid-inspect-', 'liquid-section-', 'liquid-business-'))

    @staticmethod
    def parameter(control_id):
        return bool(re.fullmatch(r'liquid-(budget|flow|damping|adhesion|smoothing)-(less|more|value)', control_id)) or control_id in {
            'liquid-slow', 'liquid-reset', 'liquid-rules-open', 'liquid-parameters-toggle', 'liquid-parameters-close',
        }

    def reveal(self, control_id):
        library = self.library(control_id)
        parameters_close = self.element('liquid-parameters-close')
        library_close = self.element('liquid-library-dialog-close')
        if self.element('liquid-parameters') and not self.parameter(control_id):
            if parameters_close and parameters_close['enabled']:
                self.click('liquid-parameters-close')
            self.wait_for_dismissal('liquid-parameters')
        if self.element('liquid-library-dialog') and not library and control_id not in {
            'liquid-library-toggle', 'liquid-library-dialog-close',
        }:
            if library_close and library_close['enabled']:
                self.click('liquid-library-dialog-close')
            self.wait_for_dismissal('liquid-library-dialog')
        control = self.element(control_id)
        variant = re.fullmatch(r'liquid-[a-z]+-variant-\d+', control_id)
        if control and (not variant or control['enabled']):
            return
        if library and self.element('liquid-library-toggle') and not self.element('liquid-library-dialog'):
            self.click('liquid-library-toggle')
        elif self.parameter(control_id) and control_id != 'liquid-parameters-toggle' and self.element('liquid-parameters-toggle') and not self.element('liquid-parameters'):
            self.click('liquid-parameters-toggle')
        elif re.fullmatch(r'liquid-[a-z]+-variant-\d+', control_id):
            selector = control_id.rsplit('-variant-', 1)[0] + '-variant-select'
            menu = self.element(selector + '-menu')
            if not menu or not menu['enabled']:
                self.click(selector)
        elif control_id.startswith('business-state-') and control_id != 'business-state-menu':
            if not self.element('business-state-menu'):
                self.click('business-state')
        if library and not control_id.startswith('liquid-section-'):
            state = self.page.evaluate('JSON.parse(zorkStory.story_state())')
            if control_id.startswith('liquid-business-') and state['section'] != 'scenarios':
                self.click('liquid-section-scenarios')
                return
            destination = next((entry for entry in state.get('catalog', [])
                                if control_id == 'liquid-inspect-' + entry['kind']
                                or control_id == 'liquid-tab-' + str(entry['group'])), None)
            if destination and state['section'] != destination['section']:
                self.click('liquid-section-' + destination['section'])

    def wait_for_dismissal(self, panel_id):
        # Automation visibility describes clipping, not occlusion. A closing
        # modal still covers the editor below it, especially during slow play.
        # Target the underlying control only after its retained panel leaves.
        self.page.wait_for_function('''id => !JSON.parse(zorkStory.snapshot()).elements.some(e => e.id === id)''', arg=panel_id)

    def locate(self, control_id):
        self.reveal(control_id)
        # Slow playback changes the number of frames needed for actionability.
        # Bound the wait by elapsed time, as browser locators do.
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            self.reveal(control_id)
            # A liquid panel exists before all of its contents fit inside its
            # live contour. Do not activate its source again or scroll through
            # the backdrop while that panel is still moving into place.
            panel_id = 'liquid-library-dialog' if self.library(control_id) else 'liquid-parameters' if self.parameter(control_id) else 'business-state-menu' if control_id.startswith('business-state-') else control_id.rsplit('-variant-', 1)[0] + '-variant-select-menu' if re.fullmatch(r'liquid-[a-z]+-variant-\d+', control_id) else None
            panel = self.element(panel_id) if panel_id else None
            if panel:
                self.frame()
                current = self.element(panel_id)
                if not current or any(abs(current['bounds'][key] - panel['bounds'][key]) >= .5 for key in ['x', 'y', 'width', 'height']):
                    continue
            element = self.element(control_id)
            menu = self.element('business-state-menu') if control_id.startswith('business-state-') else self.element(control_id.rsplit('-variant-', 1)[0] + '-variant-select-menu') if re.fullmatch(r'liquid-[a-z]+-variant-\d+', control_id) else None
            if element:
                bounds = element['bounds']
                # Main content scrolls under a fixed 52px toolbar. Popups and
                # toolbar controls have their own window-level placement.
                top = 52 if control_id.startswith('liquid-') and not control_id.endswith(('-close', '-toggle')) and '-variant-' not in control_id else 0
                visible = element['visible_bounds']
                if element['visible'] and visible['height'] >= bounds['height'] - .5 and bounds['y'] >= top and bounds['y'] + bounds['height'] < self.page.viewport_size['height']:
                    # Wheel scrolling can still be moving when an element first
                    # becomes visible. Click its settled position, as locator
                    # actionability checks do, instead of a stale snapshot.
                    self.frame()
                    settled = self.element(control_id)
                    if settled and settled['visible'] and all(abs(settled['bounds'][key] - bounds[key]) < .5 for key in ['x', 'y', 'width', 'height']):
                        return settled
                    continue
            clip_top = menu['visible_bounds']['y'] if menu else 52
            direction = -1 if element and element['bounds']['y'] < clip_top else 1
            if menu and not element and '-variant-' in control_id:
                prefix, wanted = control_id.rsplit('-variant-', 1)
                visible_indices = [int(e['id'].rsplit('-', 1)[1]) for e in self.snapshot()['elements'] if re.fullmatch(re.escape(prefix) + r'-variant-\d+', e['id'])]
                if visible_indices and int(wanted) < min(visible_indices):
                    direction = -1
            x = menu['center']['x'] if menu else element['center']['x'] if element else self.page.viewport_size['width'] / 2
            y = menu['center']['y'] if menu else self.page.viewport_size['height'] / 2
            if control_id.startswith('liquid-business-') and not element:
                directory = [e for e in self.snapshot()['elements'] if e['id'].startswith('liquid-business-') and e['visible']]
                if directory:
                    anchor = min(directory, key=lambda e: abs(e['center']['y'] - self.page.viewport_size['height'] / 2))
                    x, y = anchor['center']['x'], anchor['center']['y']
                    catalog = self.page.evaluate('JSON.parse(zorkStory.story_state()).businessCatalog')
                    order = {entry[0]: index for index, entry in enumerate(catalog)}
                    wanted = order[control_id.removeprefix('liquid-business-')]
                    direction = -1 if wanted < min(order[e['id'].removeprefix('liquid-business-')] for e in directory) else 1
            elif control_id.startswith('liquid-section-') and not element:
                directory = [e for e in self.snapshot()['elements']
                             if e['id'].startswith(('liquid-business-', 'liquid-inspect-')) and e['visible']]
                if directory:
                    anchor = min(directory, key=lambda e: abs(e['center']['y'] - self.page.viewport_size['height'] / 2))
                    x, y = anchor['center']['x'], anchor['center']['y']
                    direction = -1
            elif self.library(control_id) and not element:
                # Offscreen directory rows may not be mounted. Scroll within
                # the directory containing the visible rows, not the work area.
                directory = [e for e in self.snapshot()['elements']
                             if e['id'].startswith('liquid-inspect-') and e['visible']]
                if directory:
                    anchor = min(directory, key=lambda e: abs(e['center']['y'] - self.page.viewport_size['height'] / 2))
                    x, y = anchor['center']['x'], anchor['center']['y']
                    catalog = self.page.evaluate('JSON.parse(zorkStory.story_state()).catalog')
                    groups = [0, 1, 5, 6, 7, 8, 9, 2, 3]
                    order = {entry['kind']: index for index, entry in enumerate(
                        sorted(catalog, key=lambda e: groups.index(e['group'])))}
                    wanted = order.get(control_id.removeprefix('liquid-inspect-'))
                    if control_id.startswith('liquid-section-'):
                        wanted = -1
                    elif control_id.startswith('liquid-tab-'):
                        group = int(control_id.removeprefix('liquid-tab-'))
                        wanted = min((order[e['kind']] for e in catalog if e['group'] == group), default=len(order))
                    visible_order = [order[e['id'].removeprefix('liquid-inspect-')] for e in directory]
                    if wanted is not None:
                        direction = -1 if wanted < min(visible_order) else 1
            distance = direction * 240
            if menu and element:
                distance = element['center']['y'] - menu['center']['y']
            if self.library(control_id) and panel:
                # A row can be mounted but clipped by either edge of the
                # directory. Centre it in the observed panel instead of
                # oscillating past it in fixed, full-page wheel steps.
                x, y = panel['center']['x'], panel['center']['y']
                if element:
                    distance = element['center']['y'] - y
                elif control_id.startswith('liquid-section-'):
                    distance = -240
            self.page.mouse.move(max(8, min(self.page.viewport_size['width'] - 8, x)), y)
            self.page.mouse.wheel(0, distance)
            self.page.wait_for_timeout(70)
        raise AssertionError('Missing visible playground control ' + control_id)

    def click(self, control_id):
        element = self.locate(control_id)
        self.page.mouse.click(element['center']['x'], element['center']['y'])
        self.frame()
