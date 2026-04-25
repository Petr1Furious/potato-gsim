#include "ui/InspectorOverlay.hpp"
#include "ui/QuantityFormat.hpp"

#include <algorithm>
#include <array>
#include <cmath>
#include <filesystem>
#include <iomanip>
#include <sstream>

namespace ui {

namespace {

std::string bodyLabel(const sim::BodySnapshot& body) {
	if (!body.name.empty()) {
		return body.name;
	}
	std::ostringstream ss;
	ss << "B" << body.id;
	return ss.str();
}

sf::View makeUiView(const sf::RenderWindow& window) {
	const sf::Vector2u sz = window.getSize();
	const sf::Vector2f fsz(static_cast<float>(std::max(1u, sz.x)),
	                       static_cast<float>(std::max(1u, sz.y)));
	sf::View ui(sf::FloatRect(sf::Vector2f(0.0f, 0.0f), fsz));
	ui.setViewport(sf::FloatRect(sf::Vector2f(0.0f, 0.0f), sf::Vector2f(1.0f, 1.0f)));
	return ui;
}

}  // namespace

InspectorOverlay::InspectorOverlay() {
	fontReady_ = tryLoadFont();
}

bool InspectorOverlay::tryLoadFont() {
	static const std::array<const char*, 6> kCandidates = {
	    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
	    "/System/Library/Fonts/Supplemental/Arial.ttf",
	    "/System/Library/Fonts/SFNS.ttf",
	    "/Library/Fonts/Arial.ttf",
	    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
	    "/usr/share/fonts/TTF/DejaVuSans.ttf",
	};
	for (const char* path : kCandidates) {
		if (std::filesystem::exists(path) && font_.openFromFile(path)) {
			return true;
		}
	}
	return false;
}

void InspectorOverlay::drawArrow(sf::RenderWindow& window,
                                 const sf::Vector2f& from,
                                 const sf::Vector2f& to) const {
	sf::VertexArray shaft(sf::PrimitiveType::Lines, 2);
	shaft[0].position = from;
	shaft[0].color = sf::Color(255, 120, 120, 220);
	shaft[1].position = to;
	shaft[1].color = sf::Color(255, 120, 120, 220);
	window.draw(shaft);

	const sf::Vector2f delta = to - from;
	const float len = std::sqrt(delta.x * delta.x + delta.y * delta.y);
	if (len < 1e-3f) {
		return;
	}
	const sf::Vector2f dir = delta / len;
	const sf::Vector2f left(-dir.y, dir.x);
	const float head = std::min(24.0f, std::max(4.0f, len * 0.2f));
	sf::VertexArray tip(sf::PrimitiveType::Triangles, 3);
	tip[0].position = to;
	tip[1].position = to - dir * head + left * (head * 0.45f);
	tip[2].position = to - dir * head - left * (head * 0.45f);
	for (std::size_t i = 0; i < 3; ++i) {
		tip[i].color = sf::Color(255, 120, 120, 220);
	}
	window.draw(tip);
}

void InspectorOverlay::drawWorldSelection(
    sf::RenderWindow& window,
    const std::optional<sim::BodySnapshot>& body,
    const std::optional<sim::BodySnapshot>& velocityReferenceBody,
    const std::optional<sim::BodySnapshot>& distanceReferenceBody,
    const std::vector<sf::Vector2f>& selectedPrediction,
    bool highlightPredictionEncounterEnd,
    const std::optional<sf::Vector2f>& creationVelocityTarget,
    double arrowScale,
    bool drawHighlightSquare,
    bool showDistanceToReference,
    const std::function<sf::Vector2f(double, double)>& worldToRenderLocal,
    const std::function<sf::Vector2i(double, double)>& worldToPixel) const {
	if (!body.has_value()) {
		return;
	}
	const sf::Vector2f centerR = worldToRenderLocal(body->x, body->y);

	double relVx = body->vx;
	double relVy = body->vy;
	if (velocityReferenceBody.has_value()) {
		relVx -= velocityReferenceBody->vx;
		relVy -= velocityReferenceBody->vy;
	}
	sf::Vector2f velocityTargetR =
	    creationVelocityTarget.has_value()
	        ? worldToRenderLocal(static_cast<double>(creationVelocityTarget->x),
	                             static_cast<double>(creationVelocityTarget->y))
	        : worldToRenderLocal(body->x + relVx * arrowScale, body->y + relVy * arrowScale);
	drawArrow(window, centerR, velocityTargetR);

	if (drawHighlightSquare) {
		const sf::Vector2i centerPx = worldToPixel(body->x, body->y);
		const sf::Vector2i edgePx =
		    worldToPixel(body->x + static_cast<double>(body->radius), body->y);
		const float radiusPx = std::max(1.0f, static_cast<float>(std::abs(edgePx.x - centerPx.x)));
		const float delta = radiusPx + 5.0f;
		sf::RectangleShape box(sf::Vector2f(delta * 2.0f, delta * 2.0f));
		box.setPosition(sf::Vector2f(static_cast<float>(centerPx.x) - delta,
		                             static_cast<float>(centerPx.y) - delta));
		box.setFillColor(sf::Color::Transparent);
		box.setOutlineColor(sf::Color(255, 255, 255, 225));
		box.setOutlineThickness(1.0f);
		const sf::View oldView = window.getView();
		window.setView(makeUiView(window));
		window.draw(box);
		window.setView(oldView);
	}

	if (selectedPrediction.size() >= 2) {
		sf::VertexArray strip(sf::PrimitiveType::LineStrip, selectedPrediction.size());
		for (std::size_t i = 0; i < selectedPrediction.size(); ++i) {
			strip[i].position = worldToRenderLocal(static_cast<double>(selectedPrediction[i].x),
			                                       static_cast<double>(selectedPrediction[i].y));
			strip[i].color = sf::Color(0, 255, 0, 90);
		}
		window.draw(strip);
	}

	if (highlightPredictionEncounterEnd && !selectedPrediction.empty()) {
		const sf::Vector2f& ep = selectedPrediction.back();
		const sf::Vector2i epi = worldToPixel(static_cast<double>(ep.x), static_cast<double>(ep.y));
		const float px = static_cast<float>(epi.x);
		const float py = static_cast<float>(epi.y);
		sf::CircleShape halo(6.0f);
		halo.setOrigin(sf::Vector2f(6.0f, 6.0f));
		halo.setPosition(sf::Vector2f(px, py));
		halo.setFillColor(sf::Color(255, 80, 40, 180));
		halo.setOutlineColor(sf::Color(255, 210, 120, 230));
		halo.setOutlineThickness(1.5f);
		const sf::View oldView = window.getView();
		window.setView(makeUiView(window));
		window.draw(halo);
		window.setView(oldView);
	}

	drawBodyInfoText(window, *body, velocityReferenceBody, distanceReferenceBody,
	                 showDistanceToReference, worldToPixel);
}

void InspectorOverlay::drawBodyInfoText(
    sf::RenderWindow& window,
    const sim::BodySnapshot& body,
    const std::optional<sim::BodySnapshot>& velocityReferenceBody,
    const std::optional<sim::BodySnapshot>& distanceReferenceBody,
    bool showDistance,
    const std::function<sf::Vector2i(double, double)>& worldToPixel) const {
	if (!fontReady_) {
		return;
	}

	const sf::Vector2i centerPx = worldToPixel(body.x, body.y);
	const sf::Vector2i edgePx = worldToPixel(body.x + static_cast<double>(body.radius), body.y);
	const float radiusPx = std::max(1.0f, static_cast<float>(std::abs(edgePx.x - centerPx.x)));

	double relVx = body.vx;
	double relVy = body.vy;
	if (velocityReferenceBody.has_value()) {
		relVx -= velocityReferenceBody->vx;
		relVy -= velocityReferenceBody->vy;
	}
	double distance = 0.0;
	bool hasDistance = false;
	if (distanceReferenceBody.has_value()) {
		const double dx = body.x - distanceReferenceBody->x;
		const double dy = body.y - distanceReferenceBody->y;
		distance = std::sqrt(dx * dx + dy * dy);
		hasDistance = true;
	}
	const double speed = std::sqrt(relVx * relVx + relVy * relVy);

	std::ostringstream mass;
	mass << std::scientific << std::setprecision(3) << body.mass;

	std::string textBlock = bodyLabel(body) + "\nm=" + mass.str() + " kg" +
	                        "\nr=" + formatDistanceLegacy(body.radius) +
	                        "\nv=" + formatSpeedLegacy(speed);
	if (showDistance && hasDistance) {
		textBlock += "\nd=" + formatDistanceLegacy(distance);
	}

	sf::Text text(font_, textBlock, 12);
	text.setFillColor(sf::Color::White);
	text.setOutlineThickness(0.0f);
	const sf::FloatRect bounds = text.getLocalBounds();
	text.setOrigin(sf::Vector2f(std::round(bounds.position.x + bounds.size.x * 0.5f),
	                            std::round(bounds.position.y)));
	const float px = std::round(static_cast<float>(centerPx.x));
	const float py = std::round(static_cast<float>(centerPx.y) + radiusPx + 7.0f);
	text.setPosition(sf::Vector2f(px, py));

	const sf::View oldView = window.getView();
	window.setView(makeUiView(window));
	window.draw(text);
	window.setView(oldView);
}

void InspectorOverlay::drawLabels(sf::RenderWindow& window,
                                  const std::vector<sim::BodySnapshot>& bodies,
                                  const std::optional<sim::BodyId>& selectedId,
                                  const std::function<sf::Vector2i(double, double)>& worldToPixel,
                                  float maxScreenRadiusPx) const {
	if (!fontReady_) {
		return;
	}

	const sf::View oldView = window.getView();
	window.setView(makeUiView(window));

	struct LabelEntry {
		sf::Text text;
		sf::FloatRect bounds;
		bool priority = false;
	};

	std::vector<LabelEntry> labels;
	labels.reserve(128);
	for (const sim::BodySnapshot& body : bodies) {
		const sf::Vector2i pixel = worldToPixel(body.x, body.y);
		if (selectedId.has_value() && (*selectedId == body.id)) {
			// Selected body already has inspector text; avoid duplicate label.
			continue;
		}
		const bool highPriority = false;
		if (static_cast<float>(body.radius) > maxScreenRadiusPx) {
			continue;
		}

		sf::Text text(font_, bodyLabel(body), 12);
		text.setFillColor(highPriority ? sf::Color(255, 255, 255) : sf::Color(200, 200, 220));
		text.setOutlineColor(sf::Color(12, 12, 16, 220));
		text.setOutlineThickness(1.0f);
		text.setPosition(sf::Vector2f(std::round(static_cast<float>(pixel.x + 6)),
		                              std::round(static_cast<float>(pixel.y - 16))));
		labels.push_back(LabelEntry{
		    .text = text,
		    .bounds = text.getGlobalBounds(),
		    .priority = highPriority,
		});
	}

	std::sort(labels.begin(), labels.end(), [](const LabelEntry& a, const LabelEntry& b) {
		return static_cast<int>(a.priority) > static_cast<int>(b.priority);
	});

	std::vector<sf::FloatRect> occupied;
	occupied.reserve(labels.size());
	for (LabelEntry& label : labels) {
		std::uint8_t alpha = 220;
		for (const sf::FloatRect& other : occupied) {
			if (label.bounds.findIntersection(other).has_value()) {
				alpha = static_cast<std::uint8_t>(
				    std::min<std::uint32_t>(alpha, label.priority ? 180 : 80));
			}
		}
		sf::Color c = label.text.getFillColor();
		c.a = alpha;
		label.text.setFillColor(c);
		window.draw(label.text);
		occupied.push_back(label.bounds);
	}

	window.setView(oldView);
}

void InspectorOverlay::drawHudPanel(sf::RenderWindow& window,
                                    const std::vector<std::string>& lines,
                                    const std::vector<std::string>& legendLines,
                                    bool paused) const {
	if (!fontReady_) {
		return;
	}

	const sf::View oldView = window.getView();
	window.setView(makeUiView(window));
	const sf::Vector2f uiSize = window.getView().getSize();

	const float panelX = 12.0f;
	const float panelY = 10.0f;
	const float panelPadding = 8.0f;
	const float lineHeight = 18.0f;
	float maxLineWidth = 0.0f;
	for (const std::string& line : lines) {
		sf::Text probe(font_, line, 14);
		maxLineWidth = std::max(maxLineWidth, probe.getLocalBounds().size.x);
	}
	const float panelWidth = panelPadding * 2.0f + maxLineWidth;
	const float panelHeight = panelPadding * 2.0f + static_cast<float>(lines.size()) * lineHeight;

	sf::RectangleShape panel(sf::Vector2f(panelWidth, panelHeight));
	panel.setPosition(sf::Vector2f(panelX, panelY));
	panel.setFillColor(sf::Color(12, 16, 22, 170));
	panel.setOutlineColor(sf::Color(60, 80, 110, 220));
	panel.setOutlineThickness(1.0f);
	window.draw(panel);

	float y = panelY + panelPadding;
	for (const std::string& line : lines) {
		sf::Text text(font_, line, 14);
		text.setPosition(sf::Vector2f(panelX + panelPadding, y));
		text.setFillColor(sf::Color(230, 235, 255));
		window.draw(text);
		y += lineHeight;
	}

	if (!legendLines.empty()) {
		const float lineHeight = 18.0f;
		const float padding = 8.0f;
		float maxLineWidth = 0.0f;
		for (const std::string& line : legendLines) {
			sf::Text probe(font_, line, 13);
			maxLineWidth = std::max(maxLineWidth, probe.getLocalBounds().size.x);
		}

		const sf::Vector2f legendPanelSize(
		    maxLineWidth + padding * 2.0f,
		    padding * 2.0f + lineHeight * static_cast<float>(legendLines.size()));
		const float legendX = 12.0f;
		const float legendY = std::max(12.0f, uiSize.y - legendPanelSize.y - 12.0f);

		sf::RectangleShape legendPanel(legendPanelSize);
		legendPanel.setPosition(sf::Vector2f(legendX, legendY));
		legendPanel.setFillColor(sf::Color(12, 16, 22, 170));
		legendPanel.setOutlineColor(sf::Color(60, 80, 110, 220));
		legendPanel.setOutlineThickness(1.0f);
		window.draw(legendPanel);

		float y = legendY + padding;
		for (const std::string& line : legendLines) {
			sf::Text text(font_, line, 13);
			text.setPosition(sf::Vector2f(legendX + padding, y));
			text.setFillColor(sf::Color(190, 210, 240));
			window.draw(text);
			y += lineHeight;
		}
	}

	if (paused) {
		sf::Text pauseText(font_, "PAUSED", 34);
		pauseText.setFillColor(sf::Color(255, 225, 140, 225));
		pauseText.setOutlineColor(sf::Color(30, 30, 32));
		pauseText.setOutlineThickness(2.0f);
		pauseText.setPosition(sf::Vector2f(uiSize.x - 170.0f, 14.0f));
		window.draw(pauseText);
	}

	window.setView(oldView);
}

void InspectorOverlay::drawShipThrustHud(sf::RenderWindow& window,
                                         const int thrustPercent,
                                         const float deltaVCurrentMps,
                                         const float deltaVMaxMps) const {
	const sf::View oldView = window.getView();
	window.setView(makeUiView(window));

	const sf::Vector2f uiSize = window.getView().getSize();
	const float margin = 14.0f;
	const float colW = 16.0f;
	const float colH = 86.0f;
	const float gap = 12.0f;
	const float thrustX = uiSize.x - margin - colW;
	const float dvX = thrustX - gap - colW;
	const float y = uiSize.y - margin - colH;

	const int pct = std::clamp(thrustPercent, 0, 100);
	const float thrustFillRatio = static_cast<float>(pct) / 100.0f;
	const float dvRatio =
	    (deltaVMaxMps > 1e-3f) ? std::clamp(deltaVCurrentMps / deltaVMaxMps, 0.0f, 1.0f) : 0.0f;
	const auto drawBar = [&](const float x, const float ratio, const sf::Color& fillColor) {
		sf::RectangleShape border(sf::Vector2f(colW, colH));
		border.setPosition(sf::Vector2f(x, y));
		border.setFillColor(sf::Color(10, 14, 20, 200));
		border.setOutlineColor(sf::Color(70, 100, 140, 220));
		border.setOutlineThickness(1.0f);
		window.draw(border);
		const float fillH = colH * std::clamp(ratio, 0.0f, 1.0f);
		sf::RectangleShape fill(sf::Vector2f(colW - 3.0f, std::max(0.0f, fillH - 2.0f)));
		fill.setPosition(sf::Vector2f(x + 1.5f, y + colH - fillH + 1.0f));
		fill.setFillColor(fillColor);
		window.draw(fill);
	};
	drawBar(dvX, dvRatio, sf::Color(90, 170, 245, 230));
	drawBar(thrustX, thrustFillRatio, sf::Color(70, 210, 130, 230));

	if (fontReady_) {
		auto drawLabelAboveBar = [&](const std::string& label, const float barX,
		                             const float yOffset) {
			sf::Text text(font_, label, 15);
			text.setFillColor(sf::Color(235, 240, 255));
			const sf::FloatRect bounds = text.getLocalBounds();
			text.setOrigin(sf::Vector2f(std::round(bounds.position.x + bounds.size.x * 0.5f),
			                            std::round(bounds.position.y + bounds.size.y)));
			text.setPosition(sf::Vector2f(barX + colW * 0.5f, y - yOffset));
			window.draw(text);
		};
		drawLabelAboveBar(std::to_string(pct) + "%", thrustX, 4.0f);

		const double dvMps = std::max(0.0, static_cast<double>(deltaVCurrentMps));
		drawLabelAboveBar(formatSpeedLegacy(dvMps), dvX, 22.0f);
	}

	window.setView(oldView);
}

}  // namespace ui
